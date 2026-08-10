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
    provider::{
        contract::{
            enforce_cost, AdapterOutcome, NormalizedRequest, PricingCatalog, PricingSnapshot,
            PricingUnit, ProviderAdapter, ProviderFailure, SpendDisposition,
        },
        gemini_developer_image::{
            GeminiDeveloperImageAdapter, GeminiDeveloperTransport, ReqwestGeminiDeveloperTransport,
            ADAPTER_ID as GEMINI_DEVELOPER_ADAPTER_ID, PROVIDER_ID as GEMINI_DEVELOPER_PROVIDER_ID,
        },
        gemini_image::{
            GeminiImageAdapter, GeminiTransport, ReqwestGeminiTransport, ADAPTER_ID, PROVIDER_ID,
        },
        ideogram_image::{
            IdeogramImageAdapter, IdeogramTransport, ReqwestIdeogramTransport,
            ADAPTER_ID as IDEOGRAM_ADAPTER_ID, PROVIDER_ID as IDEOGRAM_PROVIDER_ID,
        },
        targets::ProviderTargetConfig,
    },
    secrets::{MacOsKeychain, SecretProvider},
    temporal::{
        start_worker, DurableExecutionRunner, PersistedExecutionRunner, StartedTemporalWorker,
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
use std::{future::Future, net::SocketAddr, sync::Arc};
use temporalio_client::Client;
use temporalio_sdk::Runtime;
use thiserror::Error;

const MAX_REQUEST_BYTES: usize = 1024 * 1024;

/// Production bridge from a persisted execution to the selected Gemini adapter.
/// It resolves only the execution's frozen target and credential reference and
/// never retries or falls back to another provider.
pub struct GeminiProviderActivities {
    targets: ProviderTargetConfig,
    secrets: Arc<dyn SecretProvider>,
    transport: Arc<dyn GeminiTransport>,
}

impl GeminiProviderActivities {
    pub fn production(targets: ProviderTargetConfig) -> Self {
        Self::new(
            targets,
            Arc::new(MacOsKeychain),
            Arc::new(ReqwestGeminiTransport),
        )
    }

    pub fn new(
        targets: ProviderTargetConfig,
        secrets: Arc<dyn SecretProvider>,
        transport: Arc<dyn GeminiTransport>,
    ) -> Self {
        Self {
            targets,
            secrets,
            transport,
        }
    }

    fn selected<'a>(
        &'a self,
        execution: &Execution,
    ) -> Result<
        (
            &'a crate::provider_targets::ProviderConfigVersion,
            NormalizedRequest,
            PricingSnapshot,
        ),
        WorkflowActivityError,
    > {
        if execution.provider != PROVIDER_ID || execution.adapter != ADAPTER_ID {
            return Err(WorkflowActivityError::Proven(
                "provider_target_mismatch".into(),
            ));
        }
        let key = crate::provider_targets::TargetKey::new(
            &execution.workload_type,
            &execution.provider,
            &execution.adapter,
            &execution.model,
        )
        .map_err(|_| WorkflowActivityError::Proven("provider_target_unavailable".into()))?;
        let target = self
            .targets
            .resolve_persisted_revision(
                &key,
                &execution.provider_config_version,
                &execution.provider_config_digest,
            )
            .map_err(|_| WorkflowActivityError::Proven("provider_target_unavailable".into()))?;
        let snapshot: PricingSnapshot = serde_json::from_value(execution.pricing_snapshot.clone())
            .map_err(|_| WorkflowActivityError::Proven("pricing_snapshot_invalid".into()))?;
        if !snapshot.has_unit(PricingUnit::Image) {
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
        Ok((target, request, snapshot))
    }
}

impl ProviderActivities for GeminiProviderActivities {
    fn preflight(&self, execution: &Execution) -> Result<(), WorkflowActivityError> {
        let (target, request, _) = self.selected(execution)?;
        let adapter = GeminiImageAdapter::new(
            target
                .gemini_image()
                .cloned()
                .ok_or_else(|| WorkflowActivityError::Proven("provider_config_invalid".into()))?,
            target.model.clone(),
            Arc::clone(&self.transport),
        )
        .map_err(map_contract_error)?;
        adapter
            .validate_request(&request)
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
        let (target, request, _) = self.selected(execution)?;
        let adapter = GeminiImageAdapter::new(
            target
                .gemini_image()
                .cloned()
                .ok_or_else(|| WorkflowActivityError::Proven("provider_config_invalid".into()))?,
            target.model.clone(),
            Arc::clone(&self.transport),
        )
        .map_err(map_contract_error)?;
        let secret = self
            .secrets
            .resolve(
                &target
                    .secret_reference()
                    .map_err(|_| WorkflowActivityError::Proven("secret_unavailable".into()))?,
            )
            .map_err(|_| WorkflowActivityError::Proven("secret_unavailable".into()))?;
        let outcome = adapter
            .invoke(&request, &execution.normalized_input, &secret, None)
            .map_err(map_provider_failure)?;
        normalize_provider_success(outcome)
    }
}

/// Production bridge for the API-key-authenticated Gemini Developer API.
pub struct GeminiDeveloperProviderActivities {
    targets: ProviderTargetConfig,
    secrets: Arc<dyn SecretProvider>,
    transport: Arc<dyn GeminiDeveloperTransport>,
}

impl GeminiDeveloperProviderActivities {
    pub fn production(targets: ProviderTargetConfig) -> Self {
        Self::new(
            targets,
            Arc::new(MacOsKeychain),
            Arc::new(ReqwestGeminiDeveloperTransport),
        )
    }

    pub fn new(
        targets: ProviderTargetConfig,
        secrets: Arc<dyn SecretProvider>,
        transport: Arc<dyn GeminiDeveloperTransport>,
    ) -> Self {
        Self {
            targets,
            secrets,
            transport,
        }
    }

    fn selected<'a>(
        &'a self,
        execution: &Execution,
    ) -> Result<
        (
            &'a crate::provider_targets::ProviderConfigVersion,
            NormalizedRequest,
        ),
        WorkflowActivityError,
    > {
        if execution.provider != GEMINI_DEVELOPER_PROVIDER_ID
            || execution.adapter != GEMINI_DEVELOPER_ADAPTER_ID
        {
            return Err(WorkflowActivityError::Proven(
                "provider_target_mismatch".into(),
            ));
        }
        let key = crate::provider_targets::TargetKey::new(
            &execution.workload_type,
            &execution.provider,
            &execution.adapter,
            &execution.model,
        )
        .map_err(|_| WorkflowActivityError::Proven("provider_target_unavailable".into()))?;
        let target = self
            .targets
            .resolve_persisted_revision(
                &key,
                &execution.provider_config_version,
                &execution.provider_config_digest,
            )
            .map_err(|_| WorkflowActivityError::Proven("provider_target_unavailable".into()))?;
        let snapshot: PricingSnapshot = serde_json::from_value(execution.pricing_snapshot.clone())
            .map_err(|_| WorkflowActivityError::Proven("pricing_snapshot_invalid".into()))?;
        if !snapshot.is_image_only() {
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
        Ok((
            target,
            NormalizedRequest {
                provider: snapshot.provider.clone(),
                model: snapshot.model.clone(),
                image_count: snapshot.estimated_quantity(PricingUnit::Image),
                input_tokens: None,
                max_output_tokens: None,
                image_size: snapshot
                    .selector
                    .as_ref()
                    .map(|selector| selector.image_size.clone()),
            },
        ))
    }
}

impl ProviderActivities for GeminiDeveloperProviderActivities {
    fn preflight(&self, execution: &Execution) -> Result<(), WorkflowActivityError> {
        let (target, request) = self.selected(execution)?;
        let adapter = GeminiDeveloperImageAdapter::new(
            target
                .gemini_developer_image()
                .cloned()
                .ok_or_else(|| WorkflowActivityError::Proven("provider_config_invalid".into()))?,
            target.model.clone(),
            Arc::clone(&self.transport),
        )
        .map_err(map_contract_error)?;
        adapter
            .validate_request(&request)
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
        let (target, request) = self.selected(execution)?;
        let adapter = GeminiDeveloperImageAdapter::new(
            target
                .gemini_developer_image()
                .cloned()
                .ok_or_else(|| WorkflowActivityError::Proven("provider_config_invalid".into()))?,
            target.model.clone(),
            Arc::clone(&self.transport),
        )
        .map_err(map_contract_error)?;
        let secret = self
            .secrets
            .resolve(
                &target
                    .secret_reference()
                    .map_err(|_| WorkflowActivityError::Proven("secret_unavailable".into()))?,
            )
            .map_err(|_| WorkflowActivityError::Proven("secret_unavailable".into()))?;
        let outcome = adapter
            .invoke(&request, &execution.normalized_input, &secret, None)
            .map_err(map_provider_failure)?;
        normalize_provider_success(outcome)
    }
}

/// Production bridge from a frozen Ideogram target to the shared durable
/// execution lifecycle. It never retries, recovers, or falls back.
pub struct IdeogramProviderActivities {
    targets: ProviderTargetConfig,
    secrets: Arc<dyn SecretProvider>,
    transport: Arc<dyn IdeogramTransport>,
}

impl IdeogramProviderActivities {
    pub fn production(targets: ProviderTargetConfig) -> Self {
        Self::new(
            targets,
            Arc::new(MacOsKeychain),
            Arc::new(ReqwestIdeogramTransport),
        )
    }

    pub fn new(
        targets: ProviderTargetConfig,
        secrets: Arc<dyn SecretProvider>,
        transport: Arc<dyn IdeogramTransport>,
    ) -> Self {
        Self {
            targets,
            secrets,
            transport,
        }
    }

    fn selected<'a>(
        &'a self,
        execution: &Execution,
    ) -> Result<
        (
            &'a crate::provider_targets::ProviderConfigVersion,
            NormalizedRequest,
            PricingSnapshot,
        ),
        WorkflowActivityError,
    > {
        if execution.provider != IDEOGRAM_PROVIDER_ID || execution.adapter != IDEOGRAM_ADAPTER_ID {
            return Err(WorkflowActivityError::Proven(
                "provider_target_mismatch".into(),
            ));
        }
        let key = crate::provider_targets::TargetKey::new(
            &execution.workload_type,
            &execution.provider,
            &execution.adapter,
            &execution.model,
        )
        .map_err(|_| WorkflowActivityError::Proven("provider_target_unavailable".into()))?;
        let target = self
            .targets
            .resolve_persisted_revision(
                &key,
                &execution.provider_config_version,
                &execution.provider_config_digest,
            )
            .map_err(|_| WorkflowActivityError::Proven("provider_target_unavailable".into()))?;
        let snapshot: PricingSnapshot = serde_json::from_value(execution.pricing_snapshot.clone())
            .map_err(|_| WorkflowActivityError::Proven("pricing_snapshot_invalid".into()))?;
        if !snapshot.has_unit(PricingUnit::Image) {
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
        Ok((target, request, snapshot))
    }
}

impl ProviderActivities for IdeogramProviderActivities {
    fn preflight(&self, execution: &Execution) -> Result<(), WorkflowActivityError> {
        let (target, request, _) = self.selected(execution)?;
        let adapter = IdeogramImageAdapter::new(
            target
                .ideogram_image()
                .cloned()
                .ok_or_else(|| WorkflowActivityError::Proven("provider_config_invalid".into()))?,
            target.model.clone(),
            Arc::clone(&self.transport),
        )
        .map_err(map_contract_error)?;
        adapter
            .validate_request(&request)
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
        let (target, request, _) = self.selected(execution)?;
        let adapter = IdeogramImageAdapter::new(
            target
                .ideogram_image()
                .cloned()
                .ok_or_else(|| WorkflowActivityError::Proven("provider_config_invalid".into()))?,
            target.model.clone(),
            Arc::clone(&self.transport),
        )
        .map_err(map_contract_error)?;
        let secret = self
            .secrets
            .resolve(
                &target
                    .secret_reference()
                    .map_err(|_| WorkflowActivityError::Proven("secret_unavailable".into()))?,
            )
            .map_err(|_| WorkflowActivityError::Proven("secret_unavailable".into()))?;
        let outcome = adapter
            .invoke(&request, &execution.normalized_input, &secret, None)
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

/// Compose the production Gemini and artifact activities into the durable runner.
pub fn gemini_execution_runner(
    repository: Repository,
    hubu: Arc<dyn HubuActivities + Send + Sync>,
    artifacts: ArtifactService,
    targets: ProviderTargetConfig,
    now: impl Fn() -> String + Send + Sync + Clone + 'static,
) -> Arc<dyn DurableExecutionRunner> {
    let artifact_now = now.clone();
    Arc::new(PersistedExecutionRunner::new(
        repository,
        hubu,
        Arc::new(GeminiProviderActivities::production(targets)),
        Arc::new(ArtifactServiceActivities::new(artifacts, artifact_now)),
        now,
    ))
}

/// Compose the Gemini Developer API and artifact activities into the durable runner.
pub fn gemini_developer_execution_runner(
    repository: Repository,
    hubu: Arc<dyn HubuActivities + Send + Sync>,
    artifacts: ArtifactService,
    targets: ProviderTargetConfig,
    now: impl Fn() -> String + Send + Sync + Clone + 'static,
) -> Arc<dyn DurableExecutionRunner> {
    let artifact_now = now.clone();
    Arc::new(PersistedExecutionRunner::new(
        repository,
        hubu,
        Arc::new(GeminiDeveloperProviderActivities::production(targets)),
        Arc::new(ArtifactServiceActivities::new(artifacts, artifact_now)),
        now,
    ))
}

/// Compose Ideogram and Gongbu artifact activities into the durable runner.
pub fn ideogram_execution_runner(
    repository: Repository,
    hubu: Arc<dyn HubuActivities + Send + Sync>,
    artifacts: ArtifactService,
    targets: ProviderTargetConfig,
    now: impl Fn() -> String + Send + Sync + Clone + 'static,
) -> Arc<dyn DurableExecutionRunner> {
    let artifact_now = now.clone();
    Arc::new(PersistedExecutionRunner::new(
        repository,
        hubu,
        Arc::new(IdeogramProviderActivities::production(targets)),
        Arc::new(ArtifactServiceActivities::new(artifacts, artifact_now)),
        now,
    ))
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
    pub targets: ProviderTargetConfig,
    pub pricing: PricingCatalog,
    pub temporal_runtime: Arc<Runtime>,
    pub temporal_client: Client,
    pub execution_runner: Arc<dyn DurableExecutionRunner>,
    pub authenticator: Arc<dyn Authenticator>,
    pub now: Arc<dyn Fn() -> String + Send + Sync>,
}

#[derive(Clone)]
struct ApplicationState {
    api: Api,
    authenticator: Arc<dyn Authenticator>,
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
    let mut worker = start_worker(
        dependencies.temporal_runtime,
        dependencies.temporal_client,
        dependencies.execution_runner,
    )?;
    let api = Api::new(
        dependencies.repository,
        dependencies.artifacts,
        dependencies.targets,
        dependencies.pricing,
        worker.scheduler.clone(),
        move || (dependencies.now)(),
    );
    let state = ApplicationState {
        api,
        authenticator: dependencies.authenticator,
    };
    let completion = worker.take_completion();
    let supervised_shutdown = wait_for_shutdown(shutdown, completion);
    let result = axum::serve(listener, Router::new().fallback(dispatch).with_state(state))
        .with_graceful_shutdown(supervised_shutdown)
        .await;
    stop_worker(worker)?;
    result.map_err(Into::into)
}

async fn wait_for_shutdown<F>(shutdown: F, completion: futures::channel::oneshot::Receiver<()>)
where
    F: Future<Output = ()>,
{
    match select(Box::pin(shutdown), Box::pin(completion)).await {
        Either::Left(_) | Either::Right(_) => {}
    }
}

fn stop_worker(
    worker: StartedTemporalWorker,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    worker.shutdown();
    worker.join()
}

async fn dispatch(State(state): State<ApplicationState>, request: Request<Body>) -> Response {
    let account = state.authenticator.authenticate(request.headers()).ok();
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
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
        runtime.block_on(wait_for_shutdown(futures::future::pending(), completion_rx));
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
                _: &[u8],
                _: Duration,
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
            target: "google/gemini-image-v1".into(),
            config_version: "cfg".into(),
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
        let root = tempdir().unwrap();
        let artifact_service = ArtifactService::new(
            repository.clone(),
            LocalFsStorage::new(root.path()),
            ArtifactLimits::default(),
        );
        let hubu = Arc::new(FixtureHubu::default());
        let runner = PersistedExecutionRunner::new(
            repository.clone(),
            hubu.clone(),
            Arc::new(GeminiProviderActivities::new(
                targets,
                Arc::new(FixtureSecrets),
                calls.clone(),
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
            account_id:"account".into(), operation_key:"ideogram-workflow".into(), hubu_authorization_id:"auth".into(), hubu_claim_id:Some("claim".into()), hubu_token_reference:HubuTokenReference::new("token-ref").unwrap(), authorized_minor:30, authorization_currency:"USD".into(), normalized_input:json!({"prompt":"draw a cat","image_count":1}), input_hash:"hash".into(), input_schema_version:1, target:"ideogram/ideogram-v3".into(), config_version:"cfg".into(), workload_type:"image_generation".into(), provider:"ideogram".into(), adapter:"ideogram_image".into(), model:"ideogram-v3".into(), provider_config_version:"ideogram-pcv-1".into(), provider_config_digest:targets.resolve("image_generation","ideogram","ideogram_image","ideogram-v3").unwrap().digest().to_owned(), pricing_snapshot:json!({"provider":"ideogram","model":"ideogram-v3","catalog_version":"prices-v1","catalog_digest":format!("sha256:{}", "a".repeat(64)),"pricing_rule_id":"ideogram-image","unit":"image","unit_amount_minor":30,"quantity":1,"estimated_amount_minor":30,"currency":"USD"}), pricing_schema_version:1, created_at:"now".into()
        }).unwrap();
        let root = tempdir().unwrap();
        let artifacts = ArtifactService::new(
            repository.clone(),
            LocalFsStorage::new(root.path()),
            ArtifactLimits::default(),
        );
        let hubu = Arc::new(Hubu::default());
        let runner = PersistedExecutionRunner::new(
            repository.clone(),
            hubu.clone(),
            Arc::new(IdeogramProviderActivities::new(
                targets,
                Arc::new(Secrets),
                transport.clone(),
            )),
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
