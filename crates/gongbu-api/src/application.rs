//! Runnable HTTP and Temporal process composition.
//!
//! Provider-specific activity implementations are supplied by the executable
//! that owns them. This module owns the process-level invariant that the HTTP
//! API is never served without a live Temporal worker and that both share the
//! same repository and scheduler.

use crate::{
    artifact::ArtifactService,
    execution::{Execution, Repository},
    http::{Api, AuthenticatedAccount, HttpResponse},
    lifecycle::LifecycleReason,
    provider::{
        contract::{
            enforce_cost, vendor_idempotency_key, AdapterOutcome, NormalizedRequest,
            PricingSnapshot, PricingUnit, ProviderFailure, SpendDisposition,
        },
        registry::ValidatedProviderCatalog,
    },
    secrets::{MacOsKeychain, SecretProvider},
    temporal::{
        start_worker_with_config, worker_is_polling, DurableExecutionRunner, ExecutionScheduler,
        PersistedExecutionRunner, StartedTemporalWorker, TemporalWorkerConfig,
    },
    workflow::{
        ActivityError as WorkflowActivityError, ArtifactActivities, HubuActivities,
        ProviderActivities, ProviderArtifact, ProviderSuccess,
    },
};
use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{header, HeaderMap, Request, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use futures::future::{select, Either};
use serde_json::json;
use std::{
    future::Future,
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use temporalio_client::Client;
use temporalio_sdk::Runtime;
use thiserror::Error;

const MAX_REQUEST_BYTES: usize = 1024 * 1024;

/// Generic bridge from a frozen execution to its startup-bound adapter.
/// Selection is exact: this dispatcher never routes, falls back, or invokes a
/// second provider for one execution.
pub struct GenericProviderActivities {
    providers: ValidatedProviderCatalog,
    secrets: Arc<dyn SecretProvider>,
}

impl GenericProviderActivities {
    pub fn production(providers: ValidatedProviderCatalog) -> Self {
        Self::new(providers, Arc::new(MacOsKeychain))
    }

    pub fn new(providers: ValidatedProviderCatalog, secrets: Arc<dyn SecretProvider>) -> Self {
        Self { providers, secrets }
    }

    fn selected<'a>(
        &'a self,
        execution: &Execution,
    ) -> Result<
        (
            &'a crate::provider_targets::ProviderConfigVersion,
            &'a crate::provider::registry::BoundAdapter,
            NormalizedRequest,
        ),
        WorkflowActivityError,
    > {
        let key = crate::provider_targets::TargetKey::new(
            &execution.workload_type,
            &execution.provider,
            &execution.adapter,
            &execution.model,
        )
        .map_err(|_| WorkflowActivityError::Proven("provider_target_unavailable".into()))?;
        if execution.target != key.canonical_name()
            || execution.config_version != execution.provider_config_version
        {
            return Err(WorkflowActivityError::Proven(
                "provider_target_mismatch".into(),
            ));
        }
        let (target, adapter) = self
            .providers
            .resolve_persisted(
                &key,
                &execution.provider_config_version,
                &execution.provider_config_digest,
            )
            .map_err(|_| WorkflowActivityError::Proven("provider_target_unavailable".into()))?;
        let snapshot: PricingSnapshot = serde_json::from_value(execution.pricing_snapshot.clone())
            .map_err(|_| WorkflowActivityError::Proven("pricing_snapshot_invalid".into()))?;
        if !snapshot.is_image_only()
            || snapshot.provider != target.provider
            || snapshot.model != target.model
            || i64::from(snapshot.schema_version) != execution.pricing_schema_version
        {
            return Err(WorkflowActivityError::Proven(
                "pricing_snapshot_invalid".into(),
            ));
        }
        enforce_cost(
            &snapshot,
            execution.authorized_minor,
            &execution.authorization_currency,
        )
        .map_err(|_| WorkflowActivityError::Proven("authorization_invalid".into()))?;
        let request = NormalizedRequest {
            provider: snapshot.provider.clone(),
            model: snapshot.model.clone(),
            image_count: snapshot.estimated_quantity(PricingUnit::Image),
            input_tokens: None,
            max_output_tokens: None,
            image_size: snapshot
                .selector
                .as_ref()
                .map(|selector| selector.image_size.clone()),
        };
        Ok((target, adapter, request))
    }
}

impl ProviderActivities for GenericProviderActivities {
    fn preflight(&self, execution: &Execution) -> Result<(), WorkflowActivityError> {
        let (target, adapter, request) = self.selected(execution)?;
        crate::provider_contract::validate_image_input_versioned(
            &request,
            &execution.normalized_input,
            execution.input_schema_version,
        )
        .map_err(map_contract_error)?;
        adapter
            .preflight_input(&request, &execution.normalized_input)
            .map_err(map_contract_error)?;
        self.secrets
            .resolve(
                &target
                    .secret_reference()
                    .map_err(|_| WorkflowActivityError::Proven("secret_unavailable".into()))?,
            )
            .map_err(|_| WorkflowActivityError::Proven("secret_unavailable".into()))?;
        Ok(())
    }

    fn invoke(
        &self,
        execution: &Execution,
        _attempt_id: &str,
    ) -> Result<ProviderSuccess, WorkflowActivityError> {
        let (target, adapter, request) = self.selected(execution)?;
        let secret = self
            .secrets
            .resolve(
                &target
                    .secret_reference()
                    .map_err(|_| WorkflowActivityError::Proven("secret_unavailable".into()))?,
            )
            .map_err(|_| WorkflowActivityError::Proven("secret_unavailable".into()))?;
        let idempotency_key = ValidatedProviderCatalog::needs_stable_idempotency_key(target)
            .then(|| {
                vendor_idempotency_key(
                    &target.provider,
                    &target.model,
                    &execution.account_id,
                    &execution.operation_key,
                )
            })
            .transpose()
            .map_err(map_contract_error)?;
        let outcome = adapter
            .invoke(
                &request,
                &execution.normalized_input,
                &secret,
                idempotency_key.as_deref(),
            )
            .map_err(map_provider_failure)?;
        normalize_provider_success(outcome)
    }
}

fn map_contract_error(error: crate::provider_contract::ContractError) -> WorkflowActivityError {
    let code = match error {
        crate::provider_contract::ContractError::Provider { code } => code,
        _ => "provider_contract_failure".into(),
    };
    WorkflowActivityError::Proven(code)
}

fn map_provider_failure(failure: ProviderFailure) -> WorkflowActivityError {
    match failure.spend_disposition {
        SpendDisposition::Release
            if failure.evidence.request_id.is_some() || failure.evidence.operation_id.is_some() =>
        {
            WorkflowActivityError::ProvenWithEvidence {
                code: failure.code,
                request_id: failure.evidence.request_id,
                operation_id: failure.evidence.operation_id,
            }
        }
        SpendDisposition::Release => WorkflowActivityError::Proven(failure.code),
        SpendDisposition::Reconcile
            if failure.evidence.request_id.is_some() || failure.evidence.operation_id.is_some() =>
        {
            WorkflowActivityError::AmbiguousWithEvidence {
                code: failure.code,
                request_id: failure.evidence.request_id,
                operation_id: failure.evidence.operation_id,
            }
        }
        SpendDisposition::Reconcile => WorkflowActivityError::Ambiguous(failure.code),
    }
}

fn normalize_provider_success(
    outcome: AdapterOutcome,
) -> Result<ProviderSuccess, WorkflowActivityError> {
    if outcome.validate().is_err() {
        return Err(WorkflowActivityError::AmbiguousWithEvidence {
            code: "invalid_provider_success".into(),
            request_id: outcome.provider_request_id,
            operation_id: outcome.provider_operation_id,
        });
    }
    Ok(ProviderSuccess {
        request_id: outcome.provider_request_id,
        operation_id: outcome.provider_operation_id,
        usage: outcome
            .usage
            .ok_or_else(|| WorkflowActivityError::Ambiguous("invalid_provider_success".into()))?,
        provider_amount_minor: outcome.provider_amount_minor,
        provider_currency: outcome.provider_currency,
        artifacts: outcome
            .artifacts
            .into_iter()
            .map(|artifact| ProviderArtifact {
                media_type: artifact.media_type,
                bytes: artifact.bytes,
            })
            .collect(),
    })
}

pub struct ArtifactServiceActivities {
    service: ArtifactService,
    now: Arc<dyn Fn() -> String + Send + Sync>,
}

impl ArtifactServiceActivities {
    pub fn new(service: ArtifactService, now: impl Fn() -> String + Send + Sync + 'static) -> Self {
        Self {
            service,
            now: Arc::new(now),
        }
    }
}

impl ArtifactActivities for ArtifactServiceActivities {
    fn preflight(&self) -> Result<(), WorkflowActivityError> {
        self.service
            .preflight()
            .map_err(|_| WorkflowActivityError::Proven("artifact_preflight_failed".into()))
    }

    fn persist(
        &self,
        execution: &Execution,
        attempt_id: &str,
        artifacts: &[ProviderArtifact],
    ) -> Result<(), WorkflowActivityError> {
        for artifact in artifacts {
            self.service
                .store_image(
                    &execution.execution_id,
                    Some(attempt_id),
                    &artifact.media_type,
                    &artifact.bytes,
                    &(self.now)(),
                )
                .map_err(|_| WorkflowActivityError::Proven("artifact_policy_failure".into()))?;
        }
        Ok(())
    }
}

/// Compose every startup-bound provider and artifact activities into one runner.
pub fn provider_execution_runner(
    repository: Repository,
    hubu: Arc<dyn HubuActivities + Send + Sync>,
    artifacts: ArtifactService,
    providers: ValidatedProviderCatalog,
    secrets: Arc<dyn SecretProvider>,
    now: impl Fn() -> String + Send + Sync + Clone + 'static,
) -> Arc<dyn DurableExecutionRunner> {
    let artifact_now = now.clone();
    Arc::new(PersistedExecutionRunner::new(
        repository,
        hubu,
        Arc::new(GenericProviderActivities::new(providers, secrets)),
        Arc::new(ArtifactServiceActivities::new(artifacts, artifact_now)),
        now,
    ))
}

pub fn production_provider_execution_runner(
    repository: Repository,
    hubu: Arc<dyn HubuActivities + Send + Sync>,
    artifacts: ArtifactService,
    providers: ValidatedProviderCatalog,
    now: impl Fn() -> String + Send + Sync + Clone + 'static,
) -> Arc<dyn DurableExecutionRunner> {
    provider_execution_runner(
        repository,
        hubu,
        artifacts,
        providers,
        Arc::new(MacOsKeychain),
        now,
    )
}

pub trait Authenticator: Send + Sync + 'static {
    /// Validate transport credentials and return the trusted account principal.
    fn authenticate(
        &self,
        headers: &HeaderMap,
    ) -> Result<AuthenticatedAccount, AuthenticationError>;
}

#[derive(Clone, Copy, Debug, Error)]
#[error("request authentication failed")]
pub struct AuthenticationError;

pub struct ApplicationDependencies {
    pub repository: Repository,
    pub artifacts: ArtifactService,
    pub providers: ValidatedProviderCatalog,
    pub hubu: Arc<dyn HubuActivities + Send + Sync>,
    pub secrets: Arc<dyn SecretProvider>,
    pub provider_activities: Option<Arc<dyn ProviderActivities + Send + Sync>>,
    pub artifact_activities: Option<Arc<dyn ArtifactActivities + Send + Sync>>,
    pub temporal_runtime: Arc<Runtime>,
    pub temporal_client: Client,
    pub temporal_worker: TemporalWorkerConfig,
    pub temporal_namespace: String,
    pub temporal_startup_timeout: Duration,
    pub dependency_check_interval: Duration,
    pub maximum_spend_minor: i64,
    pub dependency_checker: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    pub worker_drain_timeout: Duration,
    pub authenticator: Arc<dyn Authenticator>,
    pub now: Arc<dyn Fn() -> String + Send + Sync>,
}

#[derive(Clone)]
struct ApplicationState {
    api: Api,
    authenticator: Arc<dyn Authenticator>,
    ready: Arc<AtomicBool>,
}

/// Bind the v1 HTTP surface and poll the Temporal execution queue until shutdown.
///
/// Startup is ordered deliberately: the worker must successfully initialize
/// before the listener begins accepting execution requests.
pub async fn serve<F>(
    listener: tokio::net::TcpListener,
    dependencies: ApplicationDependencies,
    shutdown: F,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    F: Future<Output = ()> + Send + 'static,
{
    let provider = dependencies.provider_activities.unwrap_or_else(|| {
        Arc::new(GenericProviderActivities::new(
            dependencies.providers.clone(),
            dependencies.secrets,
        ))
    });
    let artifacts = dependencies.artifact_activities.unwrap_or_else(|| {
        let now = dependencies.now.clone();
        Arc::new(ArtifactServiceActivities::new(
            dependencies.artifacts.clone(),
            move || now(),
        ))
    });
    let execution_runner: Arc<dyn DurableExecutionRunner> =
        Arc::new(PersistedExecutionRunner::new(
            dependencies.repository.clone(),
            dependencies.hubu,
            provider,
            artifacts,
            {
                let now = dependencies.now.clone();
                move || now()
            },
        ));
    let temporal_client = dependencies.temporal_client.clone();
    let task_queue = dependencies.temporal_worker.task_queue.clone();
    let mut worker = start_worker_with_config(
        dependencies.temporal_runtime,
        dependencies.temporal_client,
        execution_runner,
        dependencies.temporal_worker,
    )?;
    if let Err(error) = wait_for_worker_polling(
        &temporal_client,
        &dependencies.temporal_namespace,
        &task_queue,
        dependencies.temporal_startup_timeout,
    )
    .await
    {
        worker.shutdown();
        worker.join()?;
        return Err(error);
    }
    for execution_id in dependencies.repository.list_nonterminal_execution_ids()? {
        if let Err(error) = worker.scheduler.schedule(&execution_id) {
            worker.shutdown();
            worker.join()?;
            return Err(std::io::Error::other(error).into());
        }
    }
    let api = Api::new_with_maximum_spend(
        dependencies.repository,
        dependencies.artifacts,
        dependencies.providers,
        worker.scheduler.clone(),
        dependencies.maximum_spend_minor,
        move || (dependencies.now)(),
    );
    let ready = Arc::new(AtomicBool::new(true));
    let state = ApplicationState {
        api,
        authenticator: dependencies.authenticator,
        ready: ready.clone(),
    };
    let completion = worker.take_completion();
    let supervised_ready = ready.clone();
    let supervised_shutdown = async move {
        let reason = wait_for_shutdown(
            shutdown,
            completion,
            monitor_temporal(
                temporal_client,
                dependencies.temporal_namespace,
                task_queue,
                dependencies.dependency_check_interval,
                dependencies.dependency_checker,
            ),
            supervised_ready,
        )
        .await;
        crate::lifecycle::log(reason);
    };
    let result = axum::serve(listener, Router::new().fallback(dispatch).with_state(state))
        .with_graceful_shutdown(supervised_shutdown)
        .await;
    ready.store(false, Ordering::SeqCst);
    stop_worker(worker, dependencies.worker_drain_timeout).await?;
    result.map_err(Into::into)
}

async fn wait_for_worker_polling(
    client: &Client,
    namespace: &str,
    task_queue: &str,
    timeout: Duration,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if worker_is_polling(client, namespace, task_queue)
            .await
            .unwrap_or(false)
        {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(std::io::Error::other(
                "Temporal worker did not begin polling before startup deadline",
            )
            .into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn monitor_temporal(
    client: Client,
    namespace: String,
    task_queue: String,
    interval: Duration,
    dependency_checker: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
) -> LifecycleReason {
    loop {
        tokio::time::sleep(interval).await;
        let temporal_ready = worker_is_polling(&client, &namespace, &task_queue)
            .await
            .unwrap_or(false);
        let dependencies_ready = match dependency_checker.as_ref() {
            Some(checker) => {
                let checker = checker.clone();
                tokio::task::spawn_blocking(move || checker())
                    .await
                    .unwrap_or(false)
            }
            None => true,
        };
        if !temporal_ready || !dependencies_ready {
            return LifecycleReason::DependencyHealthShutdown;
        }
    }
}

async fn wait_for_shutdown<F, H>(
    shutdown: F,
    completion: futures::channel::oneshot::Receiver<()>,
    health: H,
    ready: Arc<AtomicBool>,
) -> LifecycleReason
where
    F: Future<Output = ()>,
    H: Future<Output = LifecycleReason>,
{
    let worker_or_health = async {
        match select(Box::pin(completion), Box::pin(health)).await {
            Either::Left(_) => LifecycleReason::WorkerUnavailable,
            Either::Right((reason, _)) => reason,
        }
    };
    let reason = match select(Box::pin(shutdown), Box::pin(worker_or_health)).await {
        Either::Left(_) => LifecycleReason::OperatorSignal,
        Either::Right((reason, _)) => reason,
    };
    ready.store(false, Ordering::SeqCst);
    reason
}

async fn stop_worker(
    worker: StartedTemporalWorker,
    timeout: Duration,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    worker.shutdown();
    tokio::time::timeout(timeout, tokio::task::spawn_blocking(move || worker.join()))
        .await
        .map_err(|_| std::io::Error::other("Temporal worker drain timed out"))?
        .map_err(|_| std::io::Error::other("Temporal worker join task panicked"))??;
    Ok(())
}

async fn dispatch(State(state): State<ApplicationState>, request: Request<Body>) -> Response {
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    if method == "GET" && path == "/livez" {
        return json_transport(StatusCode::OK, json!({"status":"live"}));
    }
    if method == "GET" && path == "/readyz" {
        return readiness_response(&state.ready);
    }
    if method == "GET" && path == "/version" {
        return json_transport(
            StatusCode::OK,
            serde_json::to_value(gongbu_build_info::build_info())
                .unwrap_or_else(|_| json!({"status":"unavailable"})),
        );
    }
    if method == "POST" && path == "/v1/executions" && !state.ready.load(Ordering::SeqCst) {
        return json_transport(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"schema_version":crate::http::SCHEMA_VERSION,"error":{"code":"not_ready","message":"execution admission is temporarily unavailable"}}),
        );
    }
    let account = state.authenticator.authenticate(request.headers()).ok();
    let body = match to_bytes(request.into_body(), MAX_REQUEST_BYTES).await {
        Ok(body) => body,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let api = state.api.clone();
    match tokio::task::spawn_blocking(move || api.handle(&method, &path, account.as_ref(), &body))
        .await
    {
        Ok(response) => into_axum(response),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn readiness_response(ready: &AtomicBool) -> Response {
    let is_ready = ready.load(Ordering::SeqCst);
    json_transport(
        if is_ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        json!({"status": if is_ready { "ready" } else { "not_ready" }}),
    )
}

fn json_transport(status: StatusCode, body: serde_json::Value) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_vec(&body).unwrap_or_default(),
    )
        .into_response()
}

fn into_axum(response: HttpResponse) -> Response {
    let status = StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (
        status,
        [(header::CONTENT_TYPE, response.content_type)],
        response.body,
    )
        .into_response()
}

pub fn listener_address(listener: &tokio::net::TcpListener) -> std::io::Result<SocketAddr> {
    listener.local_addr()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        provider::{
            contract::PricingCatalog,
            gemini_image::{GeminiImageAdapter, GeminiTransport},
            ideogram_image::{IdeogramImageAdapter, IdeogramTransport},
            registry::ProviderRegistry,
        },
        provider_targets::ProviderTargetConfig,
    };

    #[test]
    fn preserves_domain_http_response_at_transport_boundary() {
        let response = into_axum(HttpResponse {
            status: 409,
            content_type: "application/json".into(),
            body: br#"{"error":"conflict"}"#.to_vec(),
        });
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let body = runtime
            .block_on(to_bytes(response.into_body(), MAX_REQUEST_BYTES))
            .unwrap();
        assert_eq!(body, br#"{"error":"conflict"}"#.as_slice());
    }

    #[test]
    fn worker_completion_triggers_http_shutdown() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (completion_tx, completion_rx) = futures::channel::oneshot::channel();
        completion_tx.send(()).unwrap();
        let ready = Arc::new(AtomicBool::new(true));
        let reason = runtime.block_on(wait_for_shutdown(
            futures::future::pending(),
            completion_rx,
            futures::future::pending(),
            ready.clone(),
        ));
        assert_eq!(reason, LifecycleReason::WorkerUnavailable);
        assert!(!ready.load(Ordering::SeqCst));
    }

    #[test]
    fn dependency_loss_removes_readiness_and_reports_shutdown_reason() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (_completion_tx, completion_rx) = futures::channel::oneshot::channel();
        let ready = Arc::new(AtomicBool::new(true));
        let reason = runtime.block_on(wait_for_shutdown(
            futures::future::pending(),
            completion_rx,
            futures::future::ready(LifecycleReason::DependencyHealthShutdown),
            ready.clone(),
        ));
        assert_eq!(reason, LifecycleReason::DependencyHealthShutdown);
        assert!(!ready.load(Ordering::SeqCst));
    }

    #[test]
    fn operator_signal_removes_readiness_and_reports_shutdown_reason() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (_completion_tx, completion_rx) = futures::channel::oneshot::channel();
        let ready = Arc::new(AtomicBool::new(true));
        let reason = runtime.block_on(wait_for_shutdown(
            futures::future::ready(()),
            completion_rx,
            futures::future::pending(),
            ready.clone(),
        ));
        assert_eq!(reason, LifecycleReason::OperatorSignal);
        assert!(!ready.load(Ordering::SeqCst));
    }

    #[test]
    fn repeated_execution_failures_leave_readiness_and_supervisor_healthy() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (_shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            let (_completion_tx, completion_rx) = futures::channel::oneshot::channel();
            let ready = Arc::new(AtomicBool::new(true));
            let mut supervisor = Box::pin(wait_for_shutdown(
                async move {
                    let _ = shutdown_rx.await;
                },
                completion_rx,
                futures::future::pending(),
                ready.clone(),
            ));

            for _ in 0..64 {
                crate::lifecycle::log(LifecycleReason::ExecutionFailure);
                assert_eq!(readiness_response(&ready).status(), StatusCode::OK);
                assert!(futures::poll!(supervisor.as_mut()).is_pending());
            }
            assert!(ready.load(Ordering::SeqCst));
        });
    }

    #[test]
    fn fixture_gemini_runs_through_durable_workflow_artifacts_and_settlement() {
        use crate::{
            artifact::{ArtifactLimits, LocalFsStorage},
            execution::{CreateExecutionParams, HubuTokenReference},
            provider::gemini_image::TransportResponse,
            secrets::{ProviderSecret, SecretError, SecretReference},
        };
        use base64::{engine::general_purpose::STANDARD, Engine};
        use image::{DynamicImage, ImageOutputFormat, RgbaImage};
        use serde_json::json;
        use std::{
            io::Cursor,
            sync::atomic::{AtomicUsize, Ordering},
            time::Duration,
        };
        use tempfile::tempdir;

        struct FixtureSecrets;
        impl SecretProvider for FixtureSecrets {
            fn resolve(&self, _: &SecretReference) -> Result<ProviderSecret, SecretError> {
                Ok(crate::secrets::secret_for_test("fixture-secret"))
            }
        }
        struct FixtureTransport(AtomicUsize, Vec<u8>);
        impl GeminiTransport for FixtureTransport {
            fn generate(
                &self,
                endpoint: &reqwest::Url,
                _: &[u8],
                _: Duration,
                _: &std::collections::BTreeMap<String, String>,
                _: &serde_json::Value,
            ) -> Result<TransportResponse, Box<dyn std::error::Error + Send + Sync>> {
                assert_eq!(endpoint.host_str(), Some("v1.googleapis.example"));
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(TransportResponse {
                    status: 200,
                    request_id: Some("google-request-1".into()),
                    operation_id: Some("google-operation-1".into()),
                    body: json!({"candidates":[{"content":{"parts":[{"inlineData":{"mimeType":"image/png","data":STANDARD.encode(&self.1)}}]}}],"usageMetadata":{"promptTokenCount":3}}),
                })
            }
            fn fetch_artifact(
                &self,
                _: &reqwest::Url,
                _: Option<&[u8]>,
                _: Duration,
                _: usize,
            ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
                unreachable!("inline fixture must not fetch a reference")
            }
        }
        #[derive(Default)]
        struct FixtureHubu {
            claims: AtomicUsize,
            settlements: AtomicUsize,
        }
        impl HubuActivities for FixtureHubu {
            fn preflight(&self, _: &Execution) -> Result<(), WorkflowActivityError> {
                Ok(())
            }
            fn claim(&self, _: &Execution) -> Result<String, WorkflowActivityError> {
                self.claims.fetch_add(1, Ordering::SeqCst);
                Ok("claim".into())
            }
            fn validate_claim(&self, _: &Execution) -> Result<(), WorkflowActivityError> {
                Ok(())
            }
            fn settle(
                &self,
                _: &Execution,
                _: &str,
                amount_minor: i64,
            ) -> Result<String, WorkflowActivityError> {
                assert_eq!(amount_minor, 25);
                self.settlements.fetch_add(1, Ordering::SeqCst);
                Ok("hubu-settlement-1".into())
            }
            fn release(&self, _: &Execution) -> Result<(), WorkflowActivityError> {
                unreachable!("happy path does not release")
            }
        }

        let mut png = Vec::new();
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255])))
            .write_to(&mut Cursor::new(&mut png), ImageOutputFormat::Png)
            .unwrap();
        let calls = Arc::new(FixtureTransport(AtomicUsize::new(0), png.clone()));
        let targets: ProviderTargetConfig = serde_json::from_value(json!({"schema_version":2,"provider_configs":[
          {"provider_config_version":"google-pcv-1","workload_type":"image_generation","provider":"google","adapter":"gemini_image","model":"gemini-image-v1","secret_service":"gongbu.google","secret_account":"fixture-v1","active":false,"execution_enabled":true,"settings":{"type":"gemini_image","config":{"endpoint":"https://v1.googleapis.example","api_version":"v1","project":"sensitive-project","location":"us-central1","timeout_ms":1000,"max_retries":0}}},
          {"provider_config_version":"google-pcv-2","workload_type":"image_generation","provider":"google","adapter":"gemini_image","model":"gemini-image-v1","secret_service":"gongbu.google","secret_account":"fixture-v2","active":true,"execution_enabled":true,"settings":{"type":"gemini_image","config":{"endpoint":"https://v2.googleapis.example","api_version":"v1","project":"sensitive-project","location":"us-central1","timeout_ms":1000,"max_retries":0}}}
        ]})).unwrap();
        targets.validate().unwrap();
        let repository = Repository::in_memory().unwrap();
        let params = CreateExecutionParams {
            account_id: "account".into(),
            operation_key: "gemini-workflow".into(),
            hubu_authorization_id: "auth".into(),
            hubu_claim_id: None,
            hubu_token_reference: HubuTokenReference::new("token-ref").unwrap(),
            authorized_minor: 25,
            authorization_currency: "USD".into(),
            normalized_input: json!({"prompt":"draw a cat","image_count":1}),
            input_hash: "hash".into(),
            input_schema_version: 1,
            target: "image_generation/google/gemini_image/gemini-image-v1".into(),
            config_version: "google-pcv-1".into(),
            workload_type: "image_generation".into(),
            provider: "google".into(),
            adapter: "gemini_image".into(),
            model: "gemini-image-v1".into(),
            provider_config_version: "google-pcv-1".into(),
            provider_config_digest: targets
                .revisions()
                .find(|revision| revision.provider_config_version == "google-pcv-1")
                .unwrap()
                .digest()
                .to_owned(),
            pricing_snapshot: json!({"provider":"google","model":"gemini-image-v1","catalog_version":"prices-v1","catalog_digest":format!("sha256:{}", "a".repeat(64)),"pricing_rule_id":"gemini-image","unit":"image","unit_amount_minor":25,"quantity":1,"estimated_amount_minor":25,"currency":"USD"}),
            pricing_schema_version: 1,
            created_at: "now".into(),
        };
        let mut mismatched = params.clone();
        mismatched.operation_key = "gemini-digest-mismatch".into();
        mismatched.provider_config_digest = format!("sha256:{}", "b".repeat(64));
        let execution = repository.create_execution(&params).unwrap();
        let mismatched = repository.create_execution(&mismatched).unwrap();
        let invalid = [
            (
                "gemini-invalid-prompt",
                json!({"prompt":"   ","image_count":1}),
            ),
            (
                "gemini-invalid-options",
                json!({"prompt":"cat","image_count":1,"options":{"seed":1}}),
            ),
        ]
        .map(|(operation_key, normalized_input)| {
            let mut invalid = params.clone();
            invalid.operation_key = operation_key.into();
            invalid.normalized_input = normalized_input;
            repository.create_execution(&invalid).unwrap()
        });
        let root = tempdir().unwrap();
        let artifact_service = ArtifactService::new(
            repository.clone(),
            LocalFsStorage::new(root.path()),
            ArtifactLimits::default(),
        );
        let hubu = Arc::new(FixtureHubu::default());
        let mut registry = ProviderRegistry::new();
        let fixture_calls = calls.clone();
        registry.register("google", "gemini_image", move |target| {
            Ok(Arc::new(GeminiImageAdapter::new(
                target.gemini_image().cloned().unwrap(),
                target.model.clone(),
                fixture_calls.clone(),
            )?))
        });
        let pricing = PricingCatalog::from_json(br#"{"schema_version":1,"catalog_version":"prices-v1","rules":[{"rule_id":"gemini-image","provider":"google","model":"gemini-image-v1","currency":"USD","unit":"image","unit_amount_minor":25}]}"#).unwrap();
        let providers = ValidatedProviderCatalog::bind(targets, pricing, &registry).unwrap();
        let runner = PersistedExecutionRunner::new(
            repository.clone(),
            hubu.clone(),
            Arc::new(GenericProviderActivities::new(
                providers,
                Arc::new(FixtureSecrets),
            )),
            Arc::new(ArtifactServiceActivities::new(
                artifact_service.clone(),
                || "now".into(),
            )),
            || "now".into(),
        );
        assert_eq!(
            runner.run_execution(&execution.execution_id).unwrap(),
            "succeeded"
        );
        assert_eq!(calls.0.load(Ordering::SeqCst), 1);
        assert_eq!(hubu.claims.load(Ordering::SeqCst), 1);
        assert_eq!(hubu.settlements.load(Ordering::SeqCst), 1);
        let attempt = repository
            .get_provider_attempt_for_execution(&execution.execution_id)
            .unwrap();
        assert_eq!(
            attempt.provider_request_id.as_deref(),
            Some("google-request-1")
        );
        assert_eq!(
            attempt.provider_operation_id.as_deref(),
            Some("google-operation-1")
        );
        let artifacts = artifact_service
            .list_for_account(&execution.execution_id, "account")
            .unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(
            artifact_service
                .retrieve_for_account(&artifacts[0].artifact_id, "account")
                .unwrap()
                .bytes,
            png
        );
        assert_eq!(
            repository
                .get_receipt_for_execution(&execution.execution_id)
                .unwrap()
                .settlement_minor,
            25
        );
        assert_eq!(
            runner.run_execution(&mismatched.execution_id).unwrap(),
            "failed"
        );
        assert_eq!(hubu.claims.load(Ordering::SeqCst), 1);
        assert_eq!(calls.0.load(Ordering::SeqCst), 1);
        for invalid in invalid {
            assert_eq!(
                runner.run_execution(&invalid.execution_id).unwrap(),
                "failed"
            );
            assert!(repository
                .get_provider_attempt_for_execution(&invalid.execution_id)
                .is_err());
        }
        assert_eq!(hubu.claims.load(Ordering::SeqCst), 1);
        assert_eq!(calls.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn fixture_ideogram_uses_frozen_price_and_durable_artifact_pipeline() {
        use crate::{
            artifact::{ArtifactLimits, LocalFsStorage},
            execution::{CreateExecutionParams, HubuTokenReference},
            provider::ideogram_image::TransportResponse,
            secrets::{ProviderSecret, SecretError, SecretReference},
        };
        use image::{DynamicImage, ImageOutputFormat, RgbaImage};
        use serde_json::json;
        use std::{
            io::Cursor,
            sync::atomic::{AtomicUsize, Ordering},
            time::Duration,
        };
        use tempfile::tempdir;

        struct Secrets;
        impl SecretProvider for Secrets {
            fn resolve(&self, _: &SecretReference) -> Result<ProviderSecret, SecretError> {
                Ok(crate::secrets::secret_for_test("ideogram-fixture-secret"))
            }
        }
        struct Transport(AtomicUsize, Vec<u8>);
        impl IdeogramTransport for Transport {
            fn generate(
                &self,
                _: &reqwest::Url,
                _: &[u8],
                _: Duration,
                _: &std::collections::BTreeMap<String, String>,
                _: &serde_json::Value,
            ) -> Result<TransportResponse, Box<dyn std::error::Error + Send + Sync>> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(TransportResponse {
                    status: 200,
                    request_id: Some("ideogram-request-1".into()),
                    operation_id: Some("ideogram-generation-1".into()),
                    body: json!({"data":[{"url":"https://ideogram.ai/output.png"}]}),
                })
            }
            fn fetch_artifact(
                &self,
                _: &reqwest::Url,
                _: Duration,
                _: usize,
            ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
                Ok(self.1.clone())
            }
        }
        #[derive(Default)]
        struct Hubu(AtomicUsize);
        impl HubuActivities for Hubu {
            fn preflight(&self, _: &Execution) -> Result<(), WorkflowActivityError> {
                Ok(())
            }
            fn claim(&self, _: &Execution) -> Result<String, WorkflowActivityError> {
                unreachable!()
            }
            fn validate_claim(&self, _: &Execution) -> Result<(), WorkflowActivityError> {
                Ok(())
            }
            fn settle(
                &self,
                _: &Execution,
                _: &str,
                amount_minor: i64,
            ) -> Result<String, WorkflowActivityError> {
                assert_eq!(amount_minor, 30);
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok("settlement-1".into())
            }
            fn release(&self, _: &Execution) -> Result<(), WorkflowActivityError> {
                unreachable!()
            }
        }

        let mut png = Vec::new();
        DynamicImage::ImageRgba8(RgbaImage::new(1, 1))
            .write_to(&mut Cursor::new(&mut png), ImageOutputFormat::Png)
            .unwrap();
        let transport = Arc::new(Transport(AtomicUsize::new(0), png.clone()));
        let targets: ProviderTargetConfig = serde_json::from_value(json!({"provider_configs":[{
            "provider_config_version":"ideogram-pcv-1","workload_type":"image_generation","provider":"ideogram","adapter":"ideogram_image","model":"ideogram-v3","secret_service":"gongbu.ideogram","secret_account":"fixture","ideogram_image":{"endpoint":"https://api.ideogram.ai","api_version":"v1","timeout_ms":1000,"max_retries":0,"approved_artifact_hosts":["ideogram.ai"]}
        }]})).unwrap();
        targets.validate().unwrap();
        let repository = Repository::in_memory().unwrap();
        let execution = repository.create_execution(&CreateExecutionParams {
            account_id:"account".into(), operation_key:"ideogram-workflow".into(), hubu_authorization_id:"auth".into(), hubu_claim_id:Some("claim".into()), hubu_token_reference:HubuTokenReference::new("token-ref").unwrap(), authorized_minor:30, authorization_currency:"USD".into(), normalized_input:json!({"prompt":"draw a cat","image_count":1}), input_hash:"hash".into(), input_schema_version:1, target:"image_generation/ideogram/ideogram_image/ideogram-v3".into(), config_version:"ideogram-pcv-1".into(), workload_type:"image_generation".into(), provider:"ideogram".into(), adapter:"ideogram_image".into(), model:"ideogram-v3".into(), provider_config_version:"ideogram-pcv-1".into(), provider_config_digest:targets.resolve("image_generation","ideogram","ideogram_image","ideogram-v3").unwrap().digest().to_owned(), pricing_snapshot:json!({"provider":"ideogram","model":"ideogram-v3","catalog_version":"prices-v1","catalog_digest":format!("sha256:{}", "a".repeat(64)),"pricing_rule_id":"ideogram-image","unit":"image","unit_amount_minor":30,"quantity":1,"estimated_amount_minor":30,"currency":"USD"}), pricing_schema_version:1, created_at:"now".into()
        }).unwrap();
        let root = tempdir().unwrap();
        let artifacts = ArtifactService::new(
            repository.clone(),
            LocalFsStorage::new(root.path()),
            ArtifactLimits::default(),
        );
        let hubu = Arc::new(Hubu::default());
        let mut registry = ProviderRegistry::new();
        let fixture_transport = transport.clone();
        registry.register("ideogram", "ideogram_image", move |target| {
            Ok(Arc::new(IdeogramImageAdapter::new(
                target.ideogram_image().cloned().unwrap(),
                target.model.clone(),
                fixture_transport.clone(),
            )?))
        });
        let pricing = PricingCatalog::from_json(br#"{"schema_version":1,"catalog_version":"prices-v1","rules":[{"rule_id":"ideogram-image","provider":"ideogram","model":"ideogram-v3","currency":"USD","unit":"image","unit_amount_minor":30}]}"#).unwrap();
        let providers = ValidatedProviderCatalog::bind(targets, pricing, &registry).unwrap();
        let runner = PersistedExecutionRunner::new(
            repository.clone(),
            hubu.clone(),
            Arc::new(GenericProviderActivities::new(providers, Arc::new(Secrets))),
            Arc::new(ArtifactServiceActivities::new(artifacts.clone(), || {
                "now".into()
            })),
            || "now".into(),
        );
        assert_eq!(
            runner.run_execution(&execution.execution_id).unwrap(),
            "succeeded"
        );
        assert_eq!(transport.0.load(Ordering::SeqCst), 1);
        assert_eq!(hubu.0.load(Ordering::SeqCst), 1);
        let attempt = repository
            .get_provider_attempt_for_execution(&execution.execution_id)
            .unwrap();
        assert_eq!(
            attempt.provider_request_id.as_deref(),
            Some("ideogram-request-1")
        );
        assert_eq!(
            attempt.provider_operation_id.as_deref(),
            Some("ideogram-generation-1")
        );
        let stored = artifacts
            .list_for_account(&execution.execution_id, "account")
            .unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(
            artifacts
                .retrieve_for_account(&stored[0].artifact_id, "account")
                .unwrap()
                .bytes,
            png
        );
        assert_eq!(
            repository
                .get_receipt_for_execution(&execution.execution_id)
                .unwrap()
                .settlement_minor,
            30
        );
    }

    #[test]
    fn mixed_provider_dispatch_is_isolated_replay_safe_and_durable() {
        use crate::{
            artifact::{ArtifactLimits, LocalFsStorage},
            execution::{CreateExecutionParams, HubuTokenReference},
            provider::contract::{
                AdapterCapabilities, AdapterOutcome, NormalizedArtifact, NormalizedUsage,
                ProviderAdapter, ProviderFailure, ProviderPhase,
            },
            secrets::{ProviderSecret, SecretError, SecretReference},
        };
        use image::{DynamicImage, ImageOutputFormat, RgbaImage};
        use serde_json::json;
        use std::{
            collections::BTreeMap,
            io::Cursor,
            sync::{
                atomic::{AtomicUsize, Ordering},
                Mutex,
            },
        };
        use tempfile::tempdir;

        struct Secrets;
        impl SecretProvider for Secrets {
            fn resolve(&self, _: &SecretReference) -> Result<ProviderSecret, SecretError> {
                Ok(crate::secrets::secret_for_test("mixed-provider-secret"))
            }
        }

        #[derive(Default)]
        struct Calls {
            counts: Mutex<BTreeMap<String, usize>>,
            flux_keys: Mutex<Vec<Option<String>>>,
        }
        struct Adapter {
            id: &'static str,
            ambiguous: bool,
            calls: Arc<Calls>,
            png: Vec<u8>,
        }
        impl ProviderAdapter for Adapter {
            fn adapter_id(&self) -> &str {
                self.id
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
                key: Option<&str>,
            ) -> Result<AdapterOutcome, ProviderFailure> {
                *self
                    .calls
                    .counts
                    .lock()
                    .unwrap()
                    .entry(self.id.into())
                    .or_default() += 1;
                if self.id == "flux2_api" {
                    self.calls
                        .flux_keys
                        .lock()
                        .unwrap()
                        .push(key.map(str::to_owned));
                }
                if self.ambiguous {
                    return Err(ProviderFailure::reconcile(
                        "timeout_unknown_outcome",
                        ProviderPhase::Processing,
                    ));
                }
                Ok(AdapterOutcome {
                    provider_request_id: Some(format!("{}-request", self.id)),
                    provider_operation_id: None,
                    usage: Some(NormalizedUsage {
                        images: Some(1),
                        input_tokens: None,
                        output_tokens: None,
                    }),
                    provider_amount_minor: None,
                    provider_currency: None,
                    artifacts: vec![NormalizedArtifact {
                        media_type: "image/png".into(),
                        bytes: self.png.clone(),
                    }],
                })
            }
        }

        #[derive(Default)]
        struct Hubu(AtomicUsize);
        impl HubuActivities for Hubu {
            fn preflight(&self, _: &Execution) -> Result<(), WorkflowActivityError> {
                Ok(())
            }
            fn claim(&self, _: &Execution) -> Result<String, WorkflowActivityError> {
                Ok("claim".into())
            }
            fn validate_claim(&self, _: &Execution) -> Result<(), WorkflowActivityError> {
                Ok(())
            }
            fn settle(
                &self,
                _: &Execution,
                _: &str,
                _: i64,
            ) -> Result<String, WorkflowActivityError> {
                let n = self.0.fetch_add(1, Ordering::SeqCst);
                Ok(format!("settlement-{n}"))
            }
            fn release(&self, _: &Execution) -> Result<(), WorkflowActivityError> {
                Ok(())
            }
        }

        let targets: ProviderTargetConfig = serde_json::from_value(json!({
            "schema_version": 2,
            "provider_configs": [
                {"provider_config_version":"google-v1","workload_type":"image_generation","provider":"google","adapter":"gemini_image","model":"gemini-image-v1","secret_service":"gongbu.google","secret_account":"mixed","active":true,"execution_enabled":true,"settings":{"type":"gemini_image","config":{"endpoint":"https://google.example","api_version":"v1","project":"project","location":"us","timeout_ms":1000}}},
                {"provider_config_version":"ideogram-v1","workload_type":"image_generation","provider":"ideogram","adapter":"ideogram_image","model":"ideogram-v3","secret_service":"gongbu.ideogram","secret_account":"mixed","active":true,"execution_enabled":true,"settings":{"type":"ideogram_image","config":{"endpoint":"https://ideogram.example","api_version":"v1","timeout_ms":1000,"approved_artifact_hosts":["ideogram.example"]}}},
                {"provider_config_version":"flux-v1","workload_type":"image_generation","provider":"flux","adapter":"flux2_api","model":"flux-2-pro","secret_service":"gongbu.flux","secret_account":"mixed","active":true,"execution_enabled":true,"settings":{"type":"flux2_api","config":{"endpoint":"https://flux.example","api_version":"v1","timeout_ms":1000,"poll_interval_ms":10,"idempotency_header":"x-idempotency-key","approved_artifact_hosts":["flux.example"]}}}
            ]
        })).unwrap();
        let pricing = PricingCatalog::from_json(br#"{"schema_version":1,"catalog_version":"mixed-v1","rules":[{"rule_id":"g","provider":"google","model":"gemini-image-v1","currency":"USD","unit":"image","unit_amount_minor":25},{"rule_id":"i","provider":"ideogram","model":"ideogram-v3","currency":"USD","unit":"image","unit_amount_minor":30},{"rule_id":"f","provider":"flux","model":"flux-2-pro","currency":"USD","unit":"image","unit_amount_minor":45}]}"#).unwrap();
        let mut png = Vec::new();
        DynamicImage::ImageRgba8(RgbaImage::new(1, 1))
            .write_to(&mut Cursor::new(&mut png), ImageOutputFormat::Png)
            .unwrap();
        let calls = Arc::new(Calls::default());
        let mut registry = ProviderRegistry::new();
        for (provider, id, ambiguous) in [
            ("google", "gemini_image", false),
            ("ideogram", "ideogram_image", true),
            ("flux", "flux2_api", false),
        ] {
            let calls = calls.clone();
            let png = png.clone();
            registry.register(provider, id, move |_| {
                Ok(Arc::new(Adapter {
                    id,
                    ambiguous,
                    calls: calls.clone(),
                    png: png.clone(),
                }))
            });
        }
        let providers =
            ValidatedProviderCatalog::bind(targets.clone(), pricing.clone(), &registry).unwrap();
        let repository = Repository::in_memory().unwrap();
        let create = |provider: &str, adapter: &str, model: &str, version: &str, amount: i64| {
            let target = targets
                .resolve("image_generation", provider, adapter, model)
                .unwrap();
            let request = NormalizedRequest {
                provider: provider.into(),
                model: model.into(),
                image_count: Some(1),
                input_tokens: None,
                max_output_tokens: None,
                image_size: None,
            };
            let snapshot = pricing
                .snapshot_for_target(&target.target_key(), &request)
                .unwrap();
            repository
                .create_execution(&CreateExecutionParams {
                    account_id: "account".into(),
                    operation_key: format!("mixed-{provider}"),
                    hubu_authorization_id: format!("auth-{provider}"),
                    hubu_claim_id: Some(format!("claim-{provider}")),
                    hubu_token_reference: HubuTokenReference::new(format!("token-{provider}"))
                        .unwrap(),
                    authorized_minor: amount,
                    authorization_currency: "USD".into(),
                    normalized_input: json!({"prompt":"draw a cat","image_count":1}),
                    input_hash: format!("hash-{provider}"),
                    input_schema_version: 1,
                    target: target.target_key().canonical_name(),
                    config_version: version.into(),
                    workload_type: "image_generation".into(),
                    provider: provider.into(),
                    adapter: adapter.into(),
                    model: model.into(),
                    provider_config_version: version.into(),
                    provider_config_digest: target.digest().into(),
                    pricing_snapshot: serde_json::to_value(&snapshot).unwrap(),
                    pricing_schema_version: i64::from(snapshot.schema_version),
                    created_at: "now".into(),
                })
                .unwrap()
        };
        let gemini = create("google", "gemini_image", "gemini-image-v1", "google-v1", 25);
        let ideogram = create(
            "ideogram",
            "ideogram_image",
            "ideogram-v3",
            "ideogram-v1",
            30,
        );
        let flux = create("flux", "flux2_api", "flux-2-pro", "flux-v1", 45);
        let root = tempdir().unwrap();
        let artifacts = ArtifactService::new(
            repository.clone(),
            LocalFsStorage::new(root.path()),
            ArtifactLimits::default(),
        );
        let hubu = Arc::new(Hubu::default());
        let runner = provider_execution_runner(
            repository.clone(),
            hubu.clone(),
            artifacts.clone(),
            providers,
            Arc::new(Secrets),
            || "now".into(),
        );

        assert_eq!(
            runner.run_execution(&gemini.execution_id, None).unwrap(),
            "succeeded"
        );
        assert_eq!(
            runner.run_execution(&ideogram.execution_id, None).unwrap(),
            "reconciliation_required"
        );
        assert_eq!(
            runner.run_execution(&flux.execution_id, None).unwrap(),
            "succeeded"
        );
        assert_eq!(
            runner.run_execution(&gemini.execution_id, None).unwrap(),
            "succeeded"
        );
        assert_eq!(
            runner.run_execution(&ideogram.execution_id, None).unwrap(),
            "reconciliation_required"
        );
        assert_eq!(
            runner.run_execution(&flux.execution_id, None).unwrap(),
            "succeeded"
        );

        let counts = calls.counts.lock().unwrap();
        assert_eq!(counts.get("gemini_image"), Some(&1));
        assert_eq!(counts.get("ideogram_image"), Some(&1));
        assert_eq!(counts.get("flux2_api"), Some(&1));
        drop(counts);
        let expected_key =
            vendor_idempotency_key("flux", "flux-2-pro", "account", "mixed-flux").unwrap();
        assert_eq!(
            calls.flux_keys.lock().unwrap().as_slice(),
            &[Some(expected_key)]
        );
        assert_eq!(hubu.0.load(Ordering::SeqCst), 2);
        assert_eq!(
            artifacts
                .list_for_account(&gemini.execution_id, "account")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            artifacts
                .list_for_account(&flux.execution_id, "account")
                .unwrap()
                .len(),
            1
        );
        assert!(artifacts
            .list_for_account(&ideogram.execution_id, "account")
            .unwrap()
            .is_empty());
        assert_eq!(
            repository
                .get_receipt_for_execution(&gemini.execution_id)
                .unwrap()
                .settlement_minor,
            25
        );
        assert_eq!(
            repository
                .get_receipt_for_execution(&flux.execution_id)
                .unwrap()
                .settlement_minor,
            45
        );
    }

    #[test]
    fn typed_spend_disposition_controls_release_independently_of_evidence() {
        use crate::provider_contract::{ProviderFailure, ProviderPhase};
        for code in [
            "timeout_unknown_outcome",
            "provider_failure",
            "malformed_response",
            "missing_image",
            "artifact_policy_failure",
        ] {
            assert_eq!(
                map_provider_failure(ProviderFailure::reconcile(code, ProviderPhase::Processing)),
                WorkflowActivityError::Ambiguous(code.into())
            );
        }
        assert_eq!(
            map_provider_failure(
                ProviderFailure::release("provider_rejected", ProviderPhase::Submission,)
                    .with_evidence(Some("request-1".into()), None)
            ),
            WorkflowActivityError::ProvenWithEvidence {
                code: "provider_rejected".into(),
                request_id: Some("request-1".into()),
                operation_id: None,
            }
        );
        assert_eq!(
            map_provider_failure(ProviderFailure::release(
                "provider_rejected_http_401",
                ProviderPhase::Submission,
            )),
            WorkflowActivityError::Proven("provider_rejected_http_401".into())
        );
        assert_eq!(
            map_provider_failure(ProviderFailure::release(
                "provider_pre_send_failure",
                ProviderPhase::PreSend,
            )),
            WorkflowActivityError::Proven("provider_pre_send_failure".into())
        );

        let invalid_success = AdapterOutcome {
            usage: Some(Default::default()),
            provider_amount_minor: Some(10),
            provider_currency: None,
            provider_request_id: Some("request-2".into()),
            provider_operation_id: Some("operation-2".into()),
            artifacts: vec![crate::provider_contract::NormalizedArtifact {
                media_type: "image/png".into(),
                bytes: vec![1],
            }],
        };
        assert!(matches!(
            normalize_provider_success(invalid_success),
            Err(WorkflowActivityError::AmbiguousWithEvidence {
                code,
                request_id: Some(request_id),
                operation_id: Some(operation_id),
            }) if code == "invalid_provider_success"
                && request_id == "request-2"
                && operation_id == "operation-2"
        ));
    }
}
