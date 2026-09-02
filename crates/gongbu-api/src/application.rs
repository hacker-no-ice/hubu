//! Runnable HTTP and Temporal process composition.
//!
//! Provider-specific activity implementations are supplied by the executable
//! that owns them. This module owns the process-level invariant that the HTTP
//! API is never served without a live Temporal worker and that both share the
//! same repository and scheduler.

use crate::{
    artifact::ArtifactService,
    execution::{Execution, Repository},
    http::{Api, AuthenticatedCaller, HttpResponse},
    hubu::SpendAuthorizationResolver,
    lifecycle::{AdmissionRoute, DependencyName, DependencyProbeOutcome, LifecycleReason},
    provider::{
        contract::{
            enforce_cost, vendor_idempotency_key, AdapterOutcome, AdapterSubmission,
            AsyncProviderOperation, NormalizedRequest, PricingSnapshot, PricingUnit,
            ProviderFailure, ProviderTransportInteraction, ProviderTransportObserver,
            SpendDisposition,
        },
        flux2_api,
        registry::ValidatedProviderCatalog,
    },
    secrets::{MacOsKeychain, SecretProvider},
    temporal::{
        start_worker_with_config, temporal_is_reachable, worker_is_polling, DurableExecutionRunner,
        ExecutionScheduler, PersistedExecutionRunner, StartedTemporalWorker, TemporalWorkerConfig,
    },
    workflow::{
        ActivityError as WorkflowActivityError, ArtifactActivities, HubuActivities,
        ProviderActivities, ProviderArtifact, ProviderSubmission, ProviderSuccess,
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
use serde_json::{json, Value};
use std::{
    convert::Infallible,
    future::Future,
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use temporalio_client::{tonic::Code, Client};
use temporalio_sdk::Runtime;
use thiserror::Error;
use tokio::time::Instant;

const MAX_REQUEST_BYTES: usize = 1024 * 1024;
pub const DEPENDENCY_FAILURE_GRACE: Duration = Duration::from_secs(30);

/// Generic bridge from a frozen execution to its startup-bound adapter.
/// Selection is exact: this dispatcher never routes, falls back, or invokes a
/// second provider for one execution.
pub struct GenericProviderActivities {
    repository: Repository,
    providers: ValidatedProviderCatalog,
    secrets: Arc<dyn SecretProvider>,
}

impl GenericProviderActivities {
    pub fn production(repository: Repository, providers: ValidatedProviderCatalog) -> Self {
        Self::new(repository, providers, Arc::new(MacOsKeychain))
    }

    pub fn new(
        repository: Repository,
        providers: ValidatedProviderCatalog,
        secrets: Arc<dyn SecretProvider>,
    ) -> Self {
        Self {
            repository,
            providers,
            secrets,
        }
    }

    fn selected<'a>(
        &'a self,
        execution: &Execution,
    ) -> Result<
        (
            &'a crate::provider_targets::ProviderConfigVersion,
            &'a crate::provider::registry::BoundAdapter,
            NormalizedRequest,
            Value,
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
        let image_size = snapshot
            .selector
            .as_ref()
            .map(|selector| selector.image_size.clone());
        let mut normalized_input = execution.normalized_input.clone();
        let output_dimensions = match snapshot.output_dimensions.clone() {
            Some(dimensions) => Some(dimensions),
            None if target.provider == flux2_api::PROVIDER_ID
                && target.adapter == flux2_api::ADAPTER_ID
                && target.model == flux2_api::MODEL_ID =>
            {
                Some(
                    flux2_api::bind_legacy_output_dimensions(
                        image_size.as_deref().ok_or_else(|| {
                            WorkflowActivityError::Proven("pricing_snapshot_invalid".into())
                        })?,
                        &mut normalized_input,
                    )
                    .map_err(map_contract_error)?,
                )
            }
            None => None,
        };
        let request = NormalizedRequest {
            provider: snapshot.provider.clone(),
            model: snapshot.model.clone(),
            image_count: snapshot.estimated_quantity(PricingUnit::Image),
            input_tokens: None,
            max_output_tokens: None,
            image_size,
            output_dimensions,
        };
        Ok((target, adapter, request, normalized_input))
    }
}

impl ProviderActivities for GenericProviderActivities {
    fn preflight(&self, execution: &Execution) -> Result<(), WorkflowActivityError> {
        let (target, adapter, request, normalized_input) = self.selected(execution)?;
        crate::provider_contract::validate_image_input_versioned(
            &request,
            &normalized_input,
            execution.input_schema_version,
        )
        .map_err(map_contract_error)?;
        adapter
            .preflight_input(&request, &normalized_input)
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
        attempt_id: &str,
    ) -> Result<ProviderSuccess, WorkflowActivityError> {
        let (target, adapter, request, normalized_input) = self.selected(execution)?;
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
            .invoke_observed(
                &request,
                &normalized_input,
                &secret,
                idempotency_key.as_deref(),
                &DurableProviderTransportObserver {
                    repository: &self.repository,
                    attempt_id,
                },
            )
            .map_err(map_provider_failure)?;
        normalize_provider_success(outcome)
    }

    fn submit(
        &self,
        execution: &Execution,
        attempt_id: &str,
    ) -> Result<ProviderSubmission, WorkflowActivityError> {
        let (target, adapter, request, normalized_input) = self.selected(execution)?;
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
        let submission = adapter
            .submit_observed(
                &request,
                &normalized_input,
                &secret,
                idempotency_key.as_deref(),
                &DurableProviderTransportObserver {
                    repository: &self.repository,
                    attempt_id,
                },
            )
            .map_err(map_provider_failure)?;
        normalize_provider_submission(submission)
    }

    fn poll_existing(
        &self,
        execution: &Execution,
        attempt_id: &str,
        operation: &AsyncProviderOperation,
    ) -> Result<ProviderSuccess, WorkflowActivityError> {
        let (target, adapter, request, normalized_input) = self
            .selected(execution)
            .map_err(|error| reconcile_local_poll_failure(error, operation))?;
        let secret = self
            .secrets
            .resolve(&target.secret_reference().map_err(|_| {
                reconcile_local_poll_failure(
                    WorkflowActivityError::Proven("secret_unavailable".into()),
                    operation,
                )
            })?)
            .map_err(|_| {
                reconcile_local_poll_failure(
                    WorkflowActivityError::Proven("secret_unavailable".into()),
                    operation,
                )
            })?;
        let outcome = adapter
            .poll_observed(
                &request,
                &normalized_input,
                &secret,
                operation,
                &DurableProviderTransportObserver {
                    repository: &self.repository,
                    attempt_id,
                },
            )
            .map_err(map_provider_failure)?;
        normalize_provider_success(outcome)
            .map_err(|error| reconcile_local_poll_failure(error, operation))
    }
}

struct DurableProviderTransportObserver<'a> {
    repository: &'a Repository,
    attempt_id: &'a str,
}

impl ProviderTransportObserver for DurableProviderTransportObserver<'_> {
    fn record(&self, interaction: ProviderTransportInteraction) -> bool {
        match interaction {
            ProviderTransportInteraction::Poll => {
                self.repository.record_provider_poll(self.attempt_id)
            }
            ProviderTransportInteraction::ArtifactFetch => {
                self.repository.record_artifact_fetch(self.attempt_id)
            }
        }
        .is_ok()
    }
}

fn reconcile_local_poll_failure(
    error: WorkflowActivityError,
    operation: &AsyncProviderOperation,
) -> WorkflowActivityError {
    let code = match error {
        WorkflowActivityError::Proven(code)
        | WorkflowActivityError::Ambiguous(code)
        | WorkflowActivityError::ProvenWithEvidence { code, .. }
        | WorkflowActivityError::AmbiguousWithEvidence { code, .. } => code,
    };
    WorkflowActivityError::AmbiguousWithEvidence {
        code,
        request_id: operation.provider_request_id.clone(),
        operation_id: Some(operation.provider_operation_id.clone()),
    }
}

fn normalize_provider_submission(
    submission: AdapterSubmission,
) -> Result<ProviderSubmission, WorkflowActivityError> {
    match submission {
        AdapterSubmission::Complete(outcome) => {
            normalize_provider_success(outcome).map(ProviderSubmission::Complete)
        }
        AdapterSubmission::Pending(operation) => {
            if operation.validate().is_err() {
                // The rejected object has not passed the compact evidence
                // allowlist, so none of its fields may enter persistence.
                return Err(WorkflowActivityError::Ambiguous(
                    "invalid_provider_operation".into(),
                ));
            }
            Ok(ProviderSubmission::Pending(operation))
        }
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
        return Err(WorkflowActivityError::Ambiguous(
            "invalid_provider_success".into(),
        ));
    }
    Ok(ProviderSuccess {
        request_id: outcome.provider_request_id,
        operation_id: outcome.provider_operation_id,
        usage: outcome
            .usage
            .ok_or_else(|| WorkflowActivityError::Ambiguous("invalid_provider_success".into()))?,
        actual_vendor_cost: outcome.actual_vendor_cost,
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
        repository.clone(),
        hubu,
        Arc::new(GenericProviderActivities::new(
            repository, providers, secrets,
        )),
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
    /// Validate the installation-scoped service capability.
    fn authenticate(&self, headers: &HeaderMap)
        -> Result<AuthenticatedCaller, AuthenticationError>;
}

#[derive(Clone, Copy, Debug, Error)]
#[error("request authentication failed")]
pub struct AuthenticationError;

pub struct ApplicationDependencies {
    pub repository: Repository,
    pub artifacts: ArtifactService,
    pub providers: ValidatedProviderCatalog,
    pub hubu: Arc<dyn HubuActivities + Send + Sync>,
    pub hubu_authorizations: Arc<dyn SpendAuthorizationResolver + Send + Sync>,
    pub secrets: Arc<dyn SecretProvider>,
    pub provider_activities: Option<Arc<dyn ProviderActivities + Send + Sync>>,
    pub artifact_activities: Option<Arc<dyn ArtifactActivities + Send + Sync>>,
    pub temporal_runtime: Arc<Runtime>,
    pub temporal_client: Client,
    pub temporal_worker: TemporalWorkerConfig,
    pub temporal_namespace: String,
    pub temporal_startup_timeout: Duration,
    pub dependency_check_interval: Duration,
    pub dependency_failure_grace: Duration,
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
    let redaction_attestation_secrets = dependencies.secrets.clone();
    let provider = dependencies.provider_activities.unwrap_or_else(|| {
        Arc::new(GenericProviderActivities::new(
            dependencies.repository.clone(),
            dependencies.providers.clone(),
            dependencies.secrets.clone(),
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
    let api = Api::new_with_authorization_resolver(
        dependencies.repository,
        dependencies.artifacts,
        dependencies.providers,
        worker.scheduler.clone(),
        dependencies.maximum_spend_minor,
        dependencies.hubu_authorizations,
        move || (dependencies.now)(),
    )
    .with_redaction_attestation_secrets(redaction_attestation_secrets);
    let ready = Arc::new(AtomicBool::new(true));
    let state = ApplicationState {
        api,
        authenticator: dependencies.authenticator,
        ready: ready.clone(),
    };
    let completion = worker.take_completion();
    let supervised_ready = ready.clone();
    let dependency_ready = ready.clone();
    let supervised_shutdown = async move {
        let reason = wait_for_shutdown(
            shutdown,
            completion,
            monitor_temporal(
                temporal_client,
                dependencies.temporal_namespace,
                task_queue,
                dependencies.dependency_check_interval,
                dependencies.dependency_failure_grace,
                dependencies.dependency_checker,
                dependency_ready,
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
    failure_grace: Duration,
    dependency_checker: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    ready: Arc<AtomicBool>,
) -> Infallible {
    // The listener is exposed only after both Temporal polling and Hubu
    // compatibility have been positively proved during startup. Seed those
    // proofs explicitly so an inconclusive runtime cancellation can preserve,
    // but can never create, readiness.
    let mut temporal_health = DependencyHealthTracker::from_positive_proof();
    let mut hubu_health = DependencyHealthTracker::from_positive_proof();
    let probe_interval = dependency_probe_interval(interval, failure_grace);
    loop {
        tokio::time::sleep(probe_interval).await;
        let (temporal_sample, grpc_code) =
            temporal_probe_sample(temporal_is_reachable(&client, &namespace, &task_queue).await);
        record_dependency_sample(
            &mut temporal_health,
            DependencyName::Temporal,
            temporal_sample,
            grpc_code.as_deref(),
            Instant::now(),
            failure_grace,
        );
        update_dependency_readiness(&ready, &temporal_health, &hubu_health);

        let dependencies_ready = match dependency_checker.as_ref() {
            Some(checker) => {
                let checker = checker.clone();
                tokio::task::spawn_blocking(move || checker())
                    .await
                    .unwrap_or(false)
            }
            None => true,
        };
        record_dependency_sample(
            &mut hubu_health,
            DependencyName::Hubu,
            if dependencies_ready {
                DependencySample::Healthy
            } else {
                DependencySample::Unhealthy
            },
            None,
            Instant::now(),
            failure_grace,
        );
        update_dependency_readiness(&ready, &temporal_health, &hubu_health);
    }
}

fn dependency_probe_interval(interval: Duration, failure_grace: Duration) -> Duration {
    interval.min(failure_grace)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DependencySample {
    Healthy,
    Unhealthy,
    Cancelled,
    Indeterminate,
}

fn temporal_probe_sample(
    result: Result<(), temporalio_client::tonic::Status>,
) -> (DependencySample, Option<String>) {
    match result {
        Ok(()) => (DependencySample::Healthy, None),
        Err(error) => {
            let code = error.code();
            (
                if code == Code::Cancelled {
                    DependencySample::Cancelled
                } else {
                    DependencySample::Indeterminate
                },
                Some(format!("{code:?}").to_ascii_lowercase()),
            )
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DependencyHealthObservation {
    Healthy,
    Degraded {
        report_transition: bool,
        failures: u32,
    },
    Recovered {
        failures: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DependencyReadiness {
    Unproven,
    Ready,
    Withdrawn,
}

struct DependencyHealthTracker {
    readiness: DependencyReadiness,
    failure_started_at: Option<Instant>,
    consecutive_failures: u32,
}

impl DependencyHealthTracker {
    #[cfg(test)]
    fn unproven() -> Self {
        Self {
            readiness: DependencyReadiness::Unproven,
            failure_started_at: None,
            consecutive_failures: 0,
        }
    }

    fn from_positive_proof() -> Self {
        Self {
            readiness: DependencyReadiness::Ready,
            failure_started_at: None,
            consecutive_failures: 0,
        }
    }

    fn is_ready(&self) -> bool {
        self.readiness == DependencyReadiness::Ready
    }

    fn observe(
        &mut self,
        sample: DependencySample,
        now: Instant,
        failure_grace: Duration,
    ) -> DependencyHealthObservation {
        if sample == DependencySample::Healthy {
            let failures = self.consecutive_failures;
            let was_unproven = self.readiness == DependencyReadiness::Unproven;
            self.readiness = DependencyReadiness::Ready;
            self.failure_started_at = None;
            self.consecutive_failures = 0;
            return if failures == 0 || was_unproven {
                DependencyHealthObservation::Healthy
            } else {
                DependencyHealthObservation::Recovered { failures }
            };
        }

        let was_ready = self.is_ready();
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let failure_started_at = *self.failure_started_at.get_or_insert(now);
        // Only a transport cancellation may retain a prior positive proof for
        // the bounded grace period. It never promotes unproven or withdrawn
        // readiness. Confirmed dependency failures withdraw readiness at once.
        if (sample != DependencySample::Cancelled
            || now.duration_since(failure_started_at) >= failure_grace)
            && self.readiness == DependencyReadiness::Ready
        {
            self.readiness = DependencyReadiness::Withdrawn;
        }
        DependencyHealthObservation::Degraded {
            report_transition: self.consecutive_failures == 1 || (was_ready && !self.is_ready()),
            failures: self.consecutive_failures,
        }
    }
}

fn update_dependency_readiness(
    ready: &AtomicBool,
    temporal_health: &DependencyHealthTracker,
    hubu_health: &DependencyHealthTracker,
) {
    ready.store(
        temporal_health.is_ready() && hubu_health.is_ready(),
        Ordering::SeqCst,
    );
}

fn record_dependency_sample(
    tracker: &mut DependencyHealthTracker,
    dependency: DependencyName,
    sample: DependencySample,
    grpc_code: Option<&str>,
    now: Instant,
    failure_grace: Duration,
) {
    let observation = tracker.observe(sample, now, failure_grace);
    match observation {
        DependencyHealthObservation::Healthy
        | DependencyHealthObservation::Degraded {
            report_transition: false,
            ..
        } => {}
        DependencyHealthObservation::Degraded {
            report_transition: true,
            failures,
        } => {
            crate::lifecycle::log_dependency_probe(
                dependency,
                probe_outcome(sample),
                failures,
                grpc_code,
            );
        }
        DependencyHealthObservation::Recovered { failures } => {
            crate::lifecycle::log_dependency_probe(
                dependency,
                DependencyProbeOutcome::Recovered,
                failures,
                None,
            );
        }
    }
}

fn probe_outcome(sample: DependencySample) -> DependencyProbeOutcome {
    match sample {
        DependencySample::Healthy => DependencyProbeOutcome::Recovered,
        DependencySample::Unhealthy => DependencyProbeOutcome::Unhealthy,
        DependencySample::Cancelled | DependencySample::Indeterminate => {
            DependencyProbeOutcome::Indeterminate
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
    H: Future<Output = Infallible>,
{
    let worker_or_health = async {
        match select(Box::pin(completion), Box::pin(health)).await {
            Either::Left(_) => LifecycleReason::WorkerUnavailable,
            Either::Right((never, _)) => match never {},
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
    if method == "POST" && path == "/v2/executions" && !state.ready.load(Ordering::SeqCst) {
        let schema_version = crate::http::SCHEMA_VERSION;
        return json_transport(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"schema_version":schema_version,"error":{"code":"not_ready","message":"execution admission is temporarily unavailable"}}),
        );
    }
    let admission_route = match (method.as_str(), path.as_str()) {
        ("POST", "/v2/executions") => Some(AdmissionRoute::CreateExecutionV2),
        _ => None,
    };
    let account = state.authenticator.authenticate(request.headers()).ok();
    let body = match to_bytes(request.into_body(), MAX_REQUEST_BYTES).await {
        Ok(body) => body,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let api = state.api.clone();
    match tokio::task::spawn_blocking(move || {
        let response = api.handle(&method, &path, account.as_ref(), &body);
        if let Some(route) = admission_route {
            crate::lifecycle::log_admission_rejection(route, response.status, &response.body);
        }
        response
    })
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
            gemini_developer_image::{GeminiDeveloperImageAdapter, GeminiDeveloperTransport},
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
    fn dependency_loss_does_not_complete_the_process_supervisor() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            let (_completion_tx, completion_rx) = futures::channel::oneshot::channel();
            let ready = Arc::new(AtomicBool::new(true));
            let mut supervisor = Box::pin(wait_for_shutdown(
                async move {
                    let _ = shutdown_rx.await;
                },
                completion_rx,
                futures::future::pending::<Infallible>(),
                ready.clone(),
            ));

            ready.store(false, Ordering::SeqCst);
            assert!(futures::poll!(supervisor.as_mut()).is_pending());
            shutdown_tx.send(()).unwrap();
            assert_eq!(supervisor.await, LifecycleReason::OperatorSignal);
        });
    }

    #[test]
    fn initial_cancelled_probe_cannot_create_readiness() {
        let mut tracker = DependencyHealthTracker::unproven();
        let started = Instant::now();
        let grace = Duration::from_secs(30);
        assert_eq!(
            tracker.observe(DependencySample::Cancelled, started, grace),
            DependencyHealthObservation::Degraded {
                report_transition: true,
                failures: 1,
            }
        );
        assert!(!tracker.is_ready());
        assert_eq!(
            tracker.observe(
                DependencySample::Cancelled,
                started + Duration::from_secs(1),
                grace,
            ),
            DependencyHealthObservation::Degraded {
                report_transition: false,
                failures: 2,
            }
        );
        assert!(!tracker.is_ready());
        assert_eq!(
            tracker.observe(
                DependencySample::Healthy,
                started + Duration::from_secs(2),
                grace,
            ),
            DependencyHealthObservation::Healthy
        );
        assert!(tracker.is_ready());
        assert_eq!(tracker.failure_started_at, None);
        assert_eq!(tracker.consecutive_failures, 0);
    }

    #[test]
    fn temporal_transport_errors_distinguish_cancelled_from_unavailable() {
        assert_eq!(
            temporal_probe_sample(Ok(())),
            (DependencySample::Healthy, None)
        );
        assert_eq!(
            temporal_probe_sample(Err(temporalio_client::tonic::Status::cancelled(
                "connection rotation",
            ))),
            (DependencySample::Cancelled, Some("cancelled".into()))
        );
        assert_eq!(
            temporal_probe_sample(Err(temporalio_client::tonic::Status::unavailable(
                "dependency unavailable",
            ))),
            (DependencySample::Indeterminate, Some("unavailable".into()))
        );
    }

    #[test]
    fn positive_proof_survives_short_cancelled_sequence_and_recovers() {
        let mut tracker = DependencyHealthTracker::from_positive_proof();
        let started = Instant::now();
        let grace = Duration::from_secs(30);
        assert_eq!(
            tracker.observe(DependencySample::Cancelled, started, grace),
            DependencyHealthObservation::Degraded {
                report_transition: true,
                failures: 1,
            }
        );
        assert!(tracker.is_ready());
        assert_eq!(
            tracker.observe(
                DependencySample::Cancelled,
                started + Duration::from_secs(29),
                grace,
            ),
            DependencyHealthObservation::Degraded {
                report_transition: false,
                failures: 2,
            }
        );
        assert!(tracker.is_ready());
        assert_eq!(tracker.failure_started_at, Some(started));
        assert_eq!(tracker.consecutive_failures, 2);
        assert_eq!(
            tracker.observe(
                DependencySample::Healthy,
                started + Duration::from_millis(29_500),
                grace,
            ),
            DependencyHealthObservation::Recovered { failures: 2 }
        );
        assert!(tracker.is_ready());
        assert_eq!(tracker.failure_started_at, None);
        assert_eq!(tracker.consecutive_failures, 0);
    }

    #[test]
    fn sustained_cancelled_sequence_withdraws_readiness_at_grace_boundary() {
        let mut tracker = DependencyHealthTracker::from_positive_proof();
        let started = Instant::now();
        let grace = Duration::from_secs(30);
        assert!(matches!(
            tracker.observe(DependencySample::Cancelled, started, grace),
            DependencyHealthObservation::Degraded { failures: 1, .. }
        ));
        assert!(tracker.is_ready());
        assert!(matches!(
            tracker.observe(
                DependencySample::Cancelled,
                started + Duration::from_secs(29),
                grace,
            ),
            DependencyHealthObservation::Degraded { failures: 2, .. }
        ));
        assert!(tracker.is_ready());
        assert_eq!(
            tracker.observe(
                DependencySample::Cancelled,
                started + Duration::from_secs(30),
                grace,
            ),
            DependencyHealthObservation::Degraded {
                report_transition: true,
                failures: 3,
            }
        );
        assert!(!tracker.is_ready());
        assert_eq!(
            tracker.observe(
                DependencySample::Cancelled,
                started + Duration::from_secs(31),
                grace,
            ),
            DependencyHealthObservation::Degraded {
                report_transition: false,
                failures: 4,
            }
        );
    }

    #[test]
    fn confirmed_dependency_failure_withdraws_readiness_immediately() {
        let started = Instant::now();
        let grace = Duration::from_secs(30);
        for sample in [DependencySample::Unhealthy, DependencySample::Indeterminate] {
            let mut tracker = DependencyHealthTracker::from_positive_proof();
            assert_eq!(
                tracker.observe(sample, started, grace),
                DependencyHealthObservation::Degraded {
                    report_transition: true,
                    failures: 1,
                }
            );
            assert!(!tracker.is_ready());
            assert_eq!(tracker.failure_started_at, Some(started));
        }
    }

    #[test]
    fn sustained_temporal_failure_stays_withdrawn_until_recovery() {
        let mut tracker = DependencyHealthTracker::from_positive_proof();
        let started = Instant::now();
        let grace = Duration::from_secs(30);
        for (offset, failures) in [(0, 1), (30, 2), (60, 3)] {
            assert_eq!(
                tracker.observe(
                    temporal_probe_sample(Err(temporalio_client::tonic::Status::unavailable(
                        "dependency unavailable",
                    )))
                    .0,
                    started + Duration::from_secs(offset),
                    grace,
                ),
                DependencyHealthObservation::Degraded {
                    report_transition: failures == 1,
                    failures,
                }
            );
            assert!(!tracker.is_ready());
        }
        assert_eq!(
            tracker.observe(
                DependencySample::Healthy,
                started + Duration::from_secs(61),
                grace,
            ),
            DependencyHealthObservation::Recovered { failures: 3 }
        );
        assert!(tracker.is_ready());
    }

    #[test]
    fn mixed_failure_samples_never_reset_or_restore_the_grace_window() {
        let mut tracker = DependencyHealthTracker::from_positive_proof();
        let started = Instant::now();
        let grace = Duration::from_secs(30);
        tracker.observe(DependencySample::Cancelled, started, grace);
        assert!(tracker.is_ready());
        assert_eq!(
            tracker.observe(
                DependencySample::Indeterminate,
                started + Duration::from_secs(5),
                grace,
            ),
            DependencyHealthObservation::Degraded {
                report_transition: true,
                failures: 2,
            }
        );
        assert!(!tracker.is_ready());
        assert_eq!(tracker.failure_started_at, Some(started));
        assert_eq!(
            tracker.observe(
                DependencySample::Cancelled,
                started + Duration::from_secs(29),
                grace,
            ),
            DependencyHealthObservation::Degraded {
                report_transition: false,
                failures: 3,
            }
        );
        assert!(!tracker.is_ready());
        assert_eq!(
            tracker.observe(
                DependencySample::Unhealthy,
                started + Duration::from_secs(30),
                grace,
            ),
            DependencyHealthObservation::Degraded {
                report_transition: false,
                failures: 4,
            }
        );
        assert!(!tracker.is_ready());
    }

    #[test]
    fn probe_interval_never_exceeds_failure_grace() {
        assert_eq!(
            dependency_probe_interval(Duration::from_secs(300), Duration::from_secs(30)),
            Duration::from_secs(30)
        );
        assert_eq!(
            dependency_probe_interval(Duration::from_secs(5), Duration::from_secs(30)),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn dependency_readiness_composes_cancelled_grace_with_fail_closed_hubu() {
        let ready = AtomicBool::new(true);
        let started = Instant::now();
        let grace = Duration::from_secs(30);
        let mut temporal_health = DependencyHealthTracker::from_positive_proof();
        let mut hubu_health = DependencyHealthTracker::from_positive_proof();

        temporal_health.observe(DependencySample::Cancelled, started, grace);
        update_dependency_readiness(&ready, &temporal_health, &hubu_health);
        assert!(ready.load(Ordering::SeqCst));

        hubu_health.observe(DependencySample::Unhealthy, started, grace);
        update_dependency_readiness(&ready, &temporal_health, &hubu_health);
        assert!(!ready.load(Ordering::SeqCst));

        hubu_health.observe(DependencySample::Healthy, started, grace);
        update_dependency_readiness(&ready, &temporal_health, &hubu_health);
        assert!(ready.load(Ordering::SeqCst));

        temporal_health.observe(
            DependencySample::Indeterminate,
            started + Duration::from_secs(1),
            grace,
        );
        update_dependency_readiness(&ready, &temporal_health, &hubu_health);
        assert!(!ready.load(Ordering::SeqCst));

        temporal_health.observe(
            DependencySample::Healthy,
            started + Duration::from_secs(2),
            grace,
        );
        update_dependency_readiness(&ready, &temporal_health, &hubu_health);
        assert!(ready.load(Ordering::SeqCst));
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
            provider::gemini_developer_image::TransportResponse,
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
        impl GeminiDeveloperTransport for FixtureTransport {
            fn create_interaction(
                &self,
                endpoint: &reqwest::Url,
                _: &[u8],
                _: Duration,
                _: &std::collections::BTreeMap<String, String>,
                _: &serde_json::Value,
            ) -> Result<TransportResponse, Box<dyn std::error::Error + Send + Sync>> {
                assert_eq!(
                    endpoint.host_str(),
                    Some("generativelanguage.googleapis.com")
                );
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(TransportResponse {
                    status: 200,
                    request_id: Some("google-request-1".into()),
                    body: json!({"id":"interaction-1","status":"completed","steps":[{"type":"model_output","content":[{"type":"image","mime_type":"image/png","data":STANDARD.encode(&self.1)}]}],"usage":{"input_tokens":3,"output_tokens":7}}),
                })
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
          {"provider_config_version":"google-pcv-1","workload_type":"image_generation","provider":"google","adapter":"gemini_developer_image","model":"gemini-3.1-flash-lite-image","secret_service":"gongbu.google","secret_account":"fixture-v1","active":false,"execution_enabled":true,"settings":{"type":"gemini_developer_image","config":{"endpoint":"https://generativelanguage.googleapis.com","api_version":"v1beta","timeout_ms":1000,"max_retries":0}}},
          {"provider_config_version":"google-pcv-2","workload_type":"image_generation","provider":"google","adapter":"gemini_developer_image","model":"gemini-3.1-flash-lite-image","secret_service":"gongbu.google","secret_account":"fixture-v2","active":true,"execution_enabled":true,"settings":{"type":"gemini_developer_image","config":{"endpoint":"https://generativelanguage.googleapis.com","api_version":"v1beta","timeout_ms":1000,"max_retries":0}}}
        ]})).unwrap();
        targets.validate().unwrap();
        let repository = Repository::in_memory().unwrap();
        let params = CreateExecutionParams {
            account_id: "account".into(),
            operation_key: "gemini-workflow".into(),
            hubu_authorization_id: "token-ref".into(),
            hubu_claim_id: None,
            hubu_token_reference: HubuTokenReference::new("token-ref").unwrap(),
            authorized_minor: 25,
            authorization_currency: "USD".into(),
            normalized_input: json!({"prompt":"draw a cat","image_count":1}),
            input_hash: "hash".into(),
            input_schema_version: 1,
            target: "image_generation/google/gemini_developer_image/gemini-3.1-flash-lite-image"
                .into(),
            config_version: "google-pcv-1".into(),
            workload_type: "image_generation".into(),
            provider: "google".into(),
            adapter: "gemini_developer_image".into(),
            model: "gemini-3.1-flash-lite-image".into(),
            provider_config_version: "google-pcv-1".into(),
            provider_config_digest: targets
                .revisions()
                .find(|revision| revision.provider_config_version == "google-pcv-1")
                .unwrap()
                .digest()
                .to_owned(),
            pricing_snapshot: json!({"schema_version":2,"provider":"google","model":"gemini-3.1-flash-lite-image","catalog_version":"prices-v2","catalog_digest":format!("sha256:{}", "a".repeat(64)),"pricing_rule_id":"gemini-developer-image","components":[{"unit":"image","rate_numerator_minor":25,"rate_denominator":1,"quantity":1}],"exact_estimate_numerator":"25","exact_estimate_denominator":"1","estimated_amount_minor":25,"currency":"USD"}),
            pricing_schema_version: 2,
            execution_scope: None,
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
        registry.register("google", "gemini_developer_image", move |target| {
            Ok(Arc::new(GeminiDeveloperImageAdapter::new(
                target.gemini_developer_image().cloned().unwrap(),
                target.model.clone(),
                fixture_calls.clone(),
            )?))
        });
        let pricing = PricingCatalog::from_json(br#"{"schema_version":2,"catalog_version":"prices-v2","rules":[{"rule_id":"gemini-developer-image","provider":"google","model":"gemini-3.1-flash-lite-image","currency":"USD","components":[{"unit":"image","rate_numerator_minor":25,"rate_denominator":1}]}]}"#).unwrap();
        let providers = ValidatedProviderCatalog::bind(targets, pricing, &registry).unwrap();
        let runner = PersistedExecutionRunner::new(
            repository.clone(),
            hubu.clone(),
            Arc::new(GenericProviderActivities::new(
                repository.clone(),
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
        assert_eq!(attempt.provider_operation_id, None);
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
            account_id:"account".into(), operation_key:"ideogram-workflow".into(), hubu_authorization_id:"token-ref".into(), hubu_claim_id:Some("claim".into()), hubu_token_reference:HubuTokenReference::new("token-ref").unwrap(), authorized_minor:30, authorization_currency:"USD".into(), normalized_input:json!({"prompt":"draw a cat","image_count":1}), input_hash:"hash".into(), input_schema_version:1, target:"image_generation/ideogram/ideogram_image/ideogram-v3".into(), config_version:"ideogram-pcv-1".into(), workload_type:"image_generation".into(), provider:"ideogram".into(), adapter:"ideogram_image".into(), model:"ideogram-v3".into(), provider_config_version:"ideogram-pcv-1".into(), provider_config_digest:targets.resolve("image_generation","ideogram","ideogram_image","ideogram-v3").unwrap().digest().to_owned(), pricing_snapshot:json!({"schema_version":2,"provider":"ideogram","model":"ideogram-v3","catalog_version":"prices-v2","catalog_digest":format!("sha256:{}", "a".repeat(64)),"pricing_rule_id":"ideogram-image","components":[{"unit":"image","rate_numerator_minor":30,"rate_denominator":1,"quantity":1}],"exact_estimate_numerator":"30","exact_estimate_denominator":"1","estimated_amount_minor":30,"currency":"USD"}), pricing_schema_version:2, execution_scope:None, created_at:"now".into()
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
        let pricing = PricingCatalog::from_json(br#"{"schema_version":2,"catalog_version":"prices-v2","rules":[{"rule_id":"ideogram-image","provider":"ideogram","model":"ideogram-v3","currency":"USD","components":[{"unit":"image","rate_numerator_minor":30,"rate_denominator":1}]}]}"#).unwrap();
        let providers = ValidatedProviderCatalog::bind(targets, pricing, &registry).unwrap();
        let runner = PersistedExecutionRunner::new(
            repository.clone(),
            hubu.clone(),
            Arc::new(GenericProviderActivities::new(
                repository.clone(),
                providers,
                Arc::new(Secrets),
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
    fn flux_tampered_dimension_evidence_fails_before_claim_attempt_or_transport() {
        use crate::{
            artifact::{ArtifactLimits, LocalFsStorage},
            execution::{CreateExecutionParams, HubuTokenReference},
            provider::{
                contract::OutputDimensions,
                flux2_api::{
                    Flux2ApiAdapter, Flux2Transport, TransportResponse as FluxTransportResponse,
                },
            },
            secrets::{ProviderSecret, SecretError, SecretReference},
        };
        use serde_json::{json, Value};
        use std::{
            collections::BTreeMap,
            error::Error as StdError,
            sync::atomic::{AtomicUsize, Ordering},
            time::Duration,
        };
        use tempfile::tempdir;

        struct Secrets;
        impl SecretProvider for Secrets {
            fn resolve(&self, _: &SecretReference) -> Result<ProviderSecret, SecretError> {
                Ok(crate::secrets::secret_for_test("flux-fixture-secret"))
            }
        }
        #[derive(Clone)]
        struct Transport(Arc<AtomicUsize>);
        impl Flux2Transport for Transport {
            fn submit(
                &self,
                _: &reqwest::Url,
                _: &[u8],
                _: Duration,
                _: &BTreeMap<String, String>,
                _: Option<(&str, &str)>,
                _: &Value,
            ) -> Result<FluxTransportResponse, Box<dyn StdError + Send + Sync>> {
                self.0.fetch_add(1, Ordering::SeqCst);
                unreachable!("invalid frozen dimensions must not reach FLUX submission")
            }
            fn poll(
                &self,
                _: &reqwest::Url,
                _: &[u8],
                _: Duration,
                _: &BTreeMap<String, String>,
            ) -> Result<FluxTransportResponse, Box<dyn StdError + Send + Sync>> {
                self.0.fetch_add(1, Ordering::SeqCst);
                unreachable!("invalid frozen dimensions must not reach FLUX polling")
            }
            fn fetch_artifact(
                &self,
                _: &reqwest::Url,
                _: Duration,
                _: usize,
            ) -> Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
                self.0.fetch_add(1, Ordering::SeqCst);
                unreachable!("invalid frozen dimensions must not fetch FLUX artifacts")
            }
        }
        #[derive(Default)]
        struct Hubu {
            claims: AtomicUsize,
        }
        impl HubuActivities for Hubu {
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
                _: i64,
            ) -> Result<String, WorkflowActivityError> {
                unreachable!()
            }
            fn release(&self, _: &Execution) -> Result<(), WorkflowActivityError> {
                Ok(())
            }
        }

        let targets: ProviderTargetConfig = serde_json::from_value(json!({
            "schema_version":2,
            "provider_configs":[{
                "provider_config_version":"flux-v1",
                "workload_type":"image_generation",
                "provider":"flux",
                "adapter":"flux2_api",
                "model":"flux-2-pro",
                "secret_service":"gongbu.flux",
                "secret_account":"fixture",
                "active":true,
                "execution_enabled":true,
                "settings":{"type":"flux2_api","config":{
                    "endpoint":"https://api.bfl.ai",
                    "api_version":"v1",
                    "timeout_ms":1000,
                    "poll_interval_ms":10,
                    "approved_artifact_hosts":["delivery.us.bfl.ai"]
                }}
            }]
        }))
        .unwrap();
        let pricing = PricingCatalog::from_json(br#"{"schema_version":2,"catalog_version":"flux-v1","rules":[{"rule_id":"flux-1k","provider":"flux","model":"flux-2-pro","selector":{"image_size":"1k"},"currency":"USD","components":[{"unit":"image","rate_numerator_minor":45,"rate_denominator":1}]},{"rule_id":"flux-2k","provider":"flux","model":"flux-2-pro","selector":{"image_size":"2k"},"currency":"USD","components":[{"unit":"image","rate_numerator_minor":90,"rate_denominator":1}]},{"rule_id":"flux-4k","provider":"flux","model":"flux-2-pro","selector":{"image_size":"4k"},"currency":"USD","components":[{"unit":"image","rate_numerator_minor":180,"rate_denominator":1}]}]}"#).unwrap();
        let request = NormalizedRequest {
            provider: "flux".into(),
            model: "flux-2-pro".into(),
            image_count: Some(1),
            input_tokens: None,
            max_output_tokens: None,
            image_size: Some("1k".into()),
            output_dimensions: Some(OutputDimensions {
                width: 1024,
                height: 1024,
            }),
        };
        let snapshot = pricing.snapshot(&request).unwrap();
        let target = targets
            .resolve("image_generation", "flux", "flux2_api", "flux-2-pro")
            .unwrap();
        let repository = Repository::in_memory().unwrap();
        let params = |operation_key: &str, input: Value, snapshot: Value| CreateExecutionParams {
            account_id: "account".into(),
            operation_key: operation_key.into(),
            hubu_authorization_id: format!("token-{operation_key}"),
            hubu_claim_id: None,
            hubu_token_reference: HubuTokenReference::new(format!("token-{operation_key}"))
                .unwrap(),
            authorized_minor: 45,
            authorization_currency: "USD".into(),
            normalized_input: input,
            input_hash: format!("hash-{operation_key}"),
            input_schema_version: 1,
            target: target.target_key().canonical_name(),
            config_version: "flux-v1".into(),
            workload_type: "image_generation".into(),
            provider: "flux".into(),
            adapter: "flux2_api".into(),
            model: "flux-2-pro".into(),
            provider_config_version: "flux-v1".into(),
            provider_config_digest: target.digest().into(),
            pricing_snapshot: snapshot,
            pricing_schema_version: 2,
            execution_scope: None,
            created_at: "now".into(),
        };
        let input_mismatch = repository
            .create_execution(&params(
                "flux-input-mismatch",
                json!({"prompt":"cat","image_count":1,"image_size":"2k","options":{"width":1920,"height":1088}}),
                serde_json::to_value(&snapshot).unwrap(),
            ))
            .unwrap();
        let current_missing_dimensions = repository
            .create_execution(&params(
                "flux-current-missing-dimensions",
                json!({"prompt":"cat","image_count":1,"image_size":"1k"}),
                serde_json::to_value(&snapshot).unwrap(),
            ))
            .unwrap();
        let mut selector_rule_mismatch = serde_json::to_value(&snapshot).unwrap();
        selector_rule_mismatch["selector"]["image_size"] = json!("2k");
        let selector_rule_mismatch = repository
            .create_execution(&params(
                "flux-selector-rule-mismatch",
                json!({"prompt":"cat","image_count":1,"image_size":"2k","options":{"width":1024,"height":1024}}),
                selector_rule_mismatch,
            ))
            .unwrap();
        let mut legacy_snapshot = serde_json::to_value(&snapshot).unwrap();
        legacy_snapshot
            .as_object_mut()
            .unwrap()
            .remove("output_dimensions");
        let legacy_conflict = repository
            .create_execution(&params(
                "flux-legacy-conflict",
                json!({"prompt":"cat","image_count":1,"image_size":"1k","options":{"width":2048,"height":2048}}),
                legacy_snapshot.clone(),
            ))
            .unwrap();
        let legacy_partial = repository
            .create_execution(&params(
                "flux-legacy-partial",
                json!({"prompt":"cat","image_count":1,"image_size":"1k","options":{"width":1024}}),
                legacy_snapshot.clone(),
            ))
            .unwrap();
        let legacy_invalid_multiple = repository
            .create_execution(&params(
                "flux-legacy-invalid-multiple",
                json!({"prompt":"cat","image_count":1,"image_size":"1k","options":{"width":1024,"height":1000}}),
                legacy_snapshot.clone(),
            ))
            .unwrap();
        let legacy_selector_mismatch = repository
            .create_execution(&params(
                "flux-legacy-selector-mismatch",
                json!({"prompt":"cat","image_count":1,"image_size":"2k"}),
                legacy_snapshot.clone(),
            ))
            .unwrap();
        let mut selectorless_legacy_snapshot = legacy_snapshot;
        selectorless_legacy_snapshot
            .as_object_mut()
            .unwrap()
            .remove("selector");
        let legacy_missing_selector = repository
            .create_execution(&params(
                "flux-legacy-missing-selector",
                json!({"prompt":"cat","image_count":1,"image_size":"1k"}),
                selectorless_legacy_snapshot,
            ))
            .unwrap();

        let transport_calls = Arc::new(AtomicUsize::new(0));
        let mut registry = ProviderRegistry::new();
        let fixture_calls = transport_calls.clone();
        registry.register("flux", "flux2_api", move |target| {
            Ok(Arc::new(Flux2ApiAdapter::new(
                target.flux2_api().cloned().unwrap(),
                target.model.clone(),
                Transport(fixture_calls.clone()),
            )?))
        });
        let providers = ValidatedProviderCatalog::bind(targets, pricing, &registry).unwrap();
        let root = tempdir().unwrap();
        let hubu = Arc::new(Hubu::default());
        let runner = PersistedExecutionRunner::new(
            repository.clone(),
            hubu.clone(),
            Arc::new(GenericProviderActivities::new(
                repository.clone(),
                providers,
                Arc::new(Secrets),
            )),
            Arc::new(ArtifactServiceActivities::new(
                ArtifactService::new(
                    repository.clone(),
                    LocalFsStorage::new(root.path()),
                    ArtifactLimits::default(),
                ),
                || "now".into(),
            )),
            || "now".into(),
        );
        for execution in [
            input_mismatch,
            current_missing_dimensions,
            selector_rule_mismatch,
            legacy_conflict,
            legacy_partial,
            legacy_invalid_multiple,
            legacy_selector_mismatch,
            legacy_missing_selector,
        ] {
            assert_eq!(
                runner.run_execution(&execution.execution_id).unwrap(),
                "failed"
            );
            assert!(repository
                .get_provider_attempt_for_execution(&execution.execution_id)
                .is_err());
        }
        assert_eq!(hubu.claims.load(Ordering::SeqCst), 0);
        assert_eq!(transport_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn managed_flux_missing_credential_and_artifact_root_fail_before_claim_or_transport() {
        use crate::{
            artifact::{ArtifactLimits, LocalFsStorage},
            execution::{CreateExecutionParams, HubuTokenReference},
            provider::{
                contract::OutputDimensions,
                flux2_api::{
                    Flux2ApiAdapter, Flux2Transport, TransportResponse as FluxTransportResponse,
                },
            },
            secrets::{ProviderSecret, SecretError, SecretReference},
        };
        use serde_json::{json, Value};
        use std::{
            collections::BTreeMap,
            error::Error as StdError,
            fs,
            sync::atomic::{AtomicUsize, Ordering},
            time::Duration,
        };
        use tempfile::tempdir;

        struct Secrets {
            calls: AtomicUsize,
            available: bool,
        }
        impl SecretProvider for Secrets {
            fn resolve(&self, reference: &SecretReference) -> Result<ProviderSecret, SecretError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(reference.service(), "gongbu.bfl.test");
                assert_eq!(reference.account(), "hub-172-fixture");
                if self.available {
                    Ok(crate::secrets::secret_for_test(
                        "synthetic-managed-flux-fixture",
                    ))
                } else {
                    Err(SecretError::Unavailable)
                }
            }
        }

        #[derive(Clone)]
        struct Transport(Arc<AtomicUsize>);
        impl Flux2Transport for Transport {
            fn submit(
                &self,
                _: &reqwest::Url,
                _: &[u8],
                _: Duration,
                _: &BTreeMap<String, String>,
                _: Option<(&str, &str)>,
                _: &Value,
            ) -> Result<FluxTransportResponse, Box<dyn StdError + Send + Sync>> {
                self.0.fetch_add(1, Ordering::SeqCst);
                unreachable!("managed FLUX preflight rejection must not submit")
            }
            fn poll(
                &self,
                _: &reqwest::Url,
                _: &[u8],
                _: Duration,
                _: &BTreeMap<String, String>,
            ) -> Result<FluxTransportResponse, Box<dyn StdError + Send + Sync>> {
                self.0.fetch_add(1, Ordering::SeqCst);
                unreachable!("managed FLUX preflight rejection must not poll")
            }
            fn fetch_artifact(
                &self,
                _: &reqwest::Url,
                _: Duration,
                _: usize,
            ) -> Result<Vec<u8>, Box<dyn StdError + Send + Sync>> {
                self.0.fetch_add(1, Ordering::SeqCst);
                unreachable!("managed FLUX preflight rejection must not fetch")
            }
        }

        #[derive(Default)]
        struct Hubu {
            preflights: AtomicUsize,
            claims: AtomicUsize,
            settlements: AtomicUsize,
            releases: AtomicUsize,
        }
        impl HubuActivities for Hubu {
            fn preflight(&self, _: &Execution) -> Result<(), WorkflowActivityError> {
                self.preflights.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            fn claim(&self, _: &Execution) -> Result<String, WorkflowActivityError> {
                self.claims.fetch_add(1, Ordering::SeqCst);
                Ok("claim-must-not-be-created".into())
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
                self.settlements.fetch_add(1, Ordering::SeqCst);
                Ok("settlement-must-not-be-created".into())
            }
            fn release(&self, _: &Execution) -> Result<(), WorkflowActivityError> {
                self.releases.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let document: Value = serde_json::from_str(include_str!(
            "../../../contracts/provider-contracts-v1.json"
        ))
        .unwrap();
        let contract_definition = document["contracts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|contract| contract["contract"] == "hubu.flux-2-pro.text-to-image/v1")
            .unwrap();
        let policies = &contract_definition["policies"];
        let mut target_document = contract_definition["target"].clone();
        target_document["secret_service"] = json!("gongbu.bfl.test");
        target_document["secret_account"] = json!("hub-172-fixture");
        let targets: ProviderTargetConfig = serde_json::from_value(json!({
            "schema_version": 3,
            "contract_bindings": [{
                "contract": contract_definition["contract"],
                "pricing_version": contract_definition["pricing_version"],
                "poll_policy": policies["poll"],
                "artifact_delivery_policy": policies["artifact_delivery"],
                "recovery_policy": policies["recovery"],
                "generation_retries": policies["generation_retries"],
                "fallback": policies["fallback"]
            }],
            "provider_configs": [target_document]
        }))
        .unwrap();
        let pricing = PricingCatalog::from_json(
            &serde_json::to_vec(&json!({
                "schema_version": 2,
                "catalog_version": contract_definition["pricing_version"],
                "rules": contract_definition["pricing_rules"]
            }))
            .unwrap(),
        )
        .unwrap();
        let target = targets
            .resolve("image_generation", "flux", "flux2_api", "flux-2-pro")
            .unwrap();
        let target_name = target.target_key().canonical_name();
        let target_version = target.provider_config_version.clone();
        let target_digest = target.digest().to_owned();
        let request = NormalizedRequest {
            provider: "flux".into(),
            model: "flux-2-pro".into(),
            image_count: Some(1),
            input_tokens: None,
            max_output_tokens: None,
            image_size: Some("1k".into()),
            output_dimensions: Some(OutputDimensions {
                width: 1024,
                height: 1024,
            }),
        };
        let snapshot = pricing
            .snapshot_for_target(&target.target_key(), &request)
            .unwrap();
        assert_eq!(target_version, "hubu-flux-2-pro-t2i-2026-08-28-v1");
        assert_eq!(snapshot.catalog_version, "bfl-flux-2-pro-usd-2026-08-28-v1");
        assert_eq!(snapshot.pricing_rule_id, "bfl-flux-2-pro-1k-2026-08-28-v1");
        assert_eq!(snapshot.estimated_amount_minor, 3);

        let transport_calls = Arc::new(AtomicUsize::new(0));
        let mut registry = ProviderRegistry::new();
        let fixture_calls = transport_calls.clone();
        registry.register("flux", "flux2_api", move |target| {
            Ok(Arc::new(Flux2ApiAdapter::new(
                target.flux2_api().cloned().unwrap(),
                target.model.clone(),
                Transport(fixture_calls.clone()),
            )?))
        });
        let providers = ValidatedProviderCatalog::bind(targets, pricing, &registry).unwrap();
        let repository = Repository::in_memory().unwrap();
        let create = |suffix: &str| {
            repository
                .create_execution(&CreateExecutionParams {
                    account_id: "account".into(),
                    operation_key: format!("qualification-{suffix}"),
                    hubu_authorization_id: format!("token-{suffix}"),
                    hubu_claim_id: None,
                    hubu_token_reference: HubuTokenReference::new(format!("token-{suffix}"))
                        .unwrap(),
                    authorized_minor: 3,
                    authorization_currency: "USD".into(),
                    normalized_input: json!({
                        "prompt":"small blue circle","image_count":1,"image_size":"1k",
                        "options":{"width":1024,"height":1024}
                    }),
                    input_hash: format!("hash-{suffix}"),
                    input_schema_version: 1,
                    target: target_name.clone(),
                    config_version: target_version.clone(),
                    workload_type: "image_generation".into(),
                    provider: "flux".into(),
                    adapter: "flux2_api".into(),
                    model: "flux-2-pro".into(),
                    provider_config_version: target_version.clone(),
                    provider_config_digest: target_digest.clone(),
                    pricing_snapshot: serde_json::to_value(&snapshot).unwrap(),
                    pricing_schema_version: i64::from(snapshot.schema_version),
                    execution_scope: None,
                    created_at: "2026-08-28T20:00:00Z".into(),
                })
                .unwrap()
        };
        let missing_credential = create("missing-credential");
        let invalid_artifact_root = create("invalid-artifact-root");
        let hubu = Arc::new(Hubu::default());

        let valid_root = tempdir().unwrap();
        let missing_secrets = Arc::new(Secrets {
            calls: AtomicUsize::new(0),
            available: false,
        });
        let missing_runner = PersistedExecutionRunner::new(
            repository.clone(),
            hubu.clone(),
            Arc::new(GenericProviderActivities::new(
                repository.clone(),
                providers.clone(),
                missing_secrets.clone(),
            )),
            Arc::new(ArtifactServiceActivities::new(
                ArtifactService::new(
                    repository.clone(),
                    LocalFsStorage::new(valid_root.path()),
                    ArtifactLimits::default(),
                ),
                || "now".into(),
            )),
            || "now".into(),
        );
        assert_eq!(
            missing_runner
                .run_execution(&missing_credential.execution_id)
                .unwrap(),
            "failed"
        );
        assert_eq!(
            repository
                .get_execution(&missing_credential.execution_id)
                .unwrap()
                .failure_code
                .as_deref(),
            Some("secret_unavailable")
        );
        assert_eq!(missing_secrets.calls.load(Ordering::SeqCst), 1);

        let blocked = tempdir().unwrap();
        let blocker = blocked.path().join("not-a-directory");
        fs::write(&blocker, b"blocked").unwrap();
        let available_secrets = Arc::new(Secrets {
            calls: AtomicUsize::new(0),
            available: true,
        });
        let artifact_runner = PersistedExecutionRunner::new(
            repository.clone(),
            hubu.clone(),
            Arc::new(GenericProviderActivities::new(
                repository.clone(),
                providers,
                available_secrets.clone(),
            )),
            Arc::new(ArtifactServiceActivities::new(
                ArtifactService::new(
                    repository.clone(),
                    LocalFsStorage::new(blocker.join("artifacts")),
                    ArtifactLimits::default(),
                ),
                || "now".into(),
            )),
            || "now".into(),
        );
        assert_eq!(
            artifact_runner
                .run_execution(&invalid_artifact_root.execution_id)
                .unwrap(),
            "failed"
        );
        assert_eq!(
            repository
                .get_execution(&invalid_artifact_root.execution_id)
                .unwrap()
                .failure_code
                .as_deref(),
            Some("artifact_preflight_failed")
        );
        assert_eq!(available_secrets.calls.load(Ordering::SeqCst), 1);

        for execution in [&missing_credential, &invalid_artifact_root] {
            assert!(repository
                .get_provider_attempt_for_execution(&execution.execution_id)
                .is_err());
            assert_eq!(
                repository
                    .count_artifacts_for_execution(&execution.execution_id)
                    .unwrap(),
                0
            );
            assert!(repository
                .get_receipt_for_execution(&execution.execution_id)
                .is_err());
        }
        assert_eq!(hubu.preflights.load(Ordering::SeqCst), 2);
        assert_eq!(hubu.claims.load(Ordering::SeqCst), 0);
        assert_eq!(hubu.settlements.load(Ordering::SeqCst), 0);
        assert_eq!(hubu.releases.load(Ordering::SeqCst), 0);
        assert_eq!(transport_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn mixed_provider_dispatch_recovers_legacy_flux_dimensions_and_is_replay_safe() {
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

        #[derive(Default)]
        struct Secrets(Mutex<Vec<(String, String)>>);
        impl SecretProvider for Secrets {
            fn resolve(&self, reference: &SecretReference) -> Result<ProviderSecret, SecretError> {
                self.0.lock().unwrap().push((
                    reference.service().to_owned(),
                    reference.account().to_owned(),
                ));
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
                request: &NormalizedRequest,
                input: &serde_json::Value,
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
                    assert_eq!(
                        request.output_dimensions,
                        Some(crate::provider_contract::OutputDimensions {
                            width: 1024,
                            height: 1024,
                        })
                    );
                    assert_eq!(input["options"]["width"], 1024);
                    assert_eq!(input["options"]["height"], 1024);
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
                    actual_vendor_cost: None,
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
                {"provider_config_version":"google-v1","workload_type":"image_generation","provider":"google","adapter":"gemini_developer_image","model":"gemini-3.1-flash-lite-image","secret_service":"gongbu.google","secret_account":"gemini","active":true,"execution_enabled":true,"settings":{"type":"gemini_developer_image","config":{"endpoint":"https://generativelanguage.googleapis.com","api_version":"v1beta","timeout_ms":1000}}},
                {"provider_config_version":"ideogram-v1","workload_type":"image_generation","provider":"ideogram","adapter":"ideogram_image","model":"ideogram-v3","secret_service":"gongbu.ideogram","secret_account":"ideogram","active":true,"execution_enabled":true,"settings":{"type":"ideogram_image","config":{"endpoint":"https://ideogram.example","api_version":"v1","timeout_ms":1000,"approved_artifact_hosts":["ideogram.example"]}}},
                {"provider_config_version":"flux-v1","workload_type":"image_generation","provider":"flux","adapter":"flux2_api","model":"flux-2-pro","secret_service":"gongbu.flux","secret_account":"flux","active":true,"execution_enabled":true,"settings":{"type":"flux2_api","config":{"endpoint":"https://api.bfl.ai","api_version":"v1","timeout_ms":1000,"poll_interval_ms":10,"idempotency_header":"x-idempotency-key","approved_artifact_hosts":["delivery.us.bfl.ai"]}}}
            ]
        })).unwrap();
        let pricing = PricingCatalog::from_json(br#"{"schema_version":2,"catalog_version":"mixed-v2","rules":[{"rule_id":"g","provider":"google","model":"gemini-3.1-flash-lite-image","currency":"USD","components":[{"unit":"image","rate_numerator_minor":25,"rate_denominator":1}]},{"rule_id":"i","provider":"ideogram","model":"ideogram-v3","currency":"USD","components":[{"unit":"image","rate_numerator_minor":30,"rate_denominator":1}]},{"rule_id":"f-1k","provider":"flux","model":"flux-2-pro","selector":{"image_size":"1k"},"currency":"USD","components":[{"unit":"image","rate_numerator_minor":45,"rate_denominator":1}]},{"rule_id":"f-2k","provider":"flux","model":"flux-2-pro","selector":{"image_size":"2k"},"currency":"USD","components":[{"unit":"image","rate_numerator_minor":90,"rate_denominator":1}]},{"rule_id":"f-4k","provider":"flux","model":"flux-2-pro","selector":{"image_size":"4k"},"currency":"USD","components":[{"unit":"image","rate_numerator_minor":180,"rate_denominator":1}]}]}"#).unwrap();
        let mut png = Vec::new();
        DynamicImage::ImageRgba8(RgbaImage::new(1, 1))
            .write_to(&mut Cursor::new(&mut png), ImageOutputFormat::Png)
            .unwrap();
        let calls = Arc::new(Calls::default());
        let mut registry = ProviderRegistry::new();
        for (provider, id, ambiguous) in [
            ("google", "gemini_developer_image", false),
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
            let is_flux = provider == "flux";
            let request = NormalizedRequest {
                provider: provider.into(),
                model: model.into(),
                image_count: Some(1),
                input_tokens: None,
                max_output_tokens: None,
                image_size: is_flux.then(|| "1k".into()),
                output_dimensions: is_flux.then_some(crate::provider_contract::OutputDimensions {
                    width: 1024,
                    height: 1024,
                }),
            };
            let snapshot = pricing
                .snapshot_for_target(&target.target_key(), &request)
                .unwrap();
            let mut pricing_snapshot = serde_json::to_value(&snapshot).unwrap();
            if is_flux {
                // Pre-HUB-168 schema-v2 rows froze the selector and caller input,
                // but did not yet copy exact dimensions into the pricing snapshot.
                pricing_snapshot
                    .as_object_mut()
                    .unwrap()
                    .remove("output_dimensions");
            }
            repository
                .create_execution(&CreateExecutionParams {
                    account_id: "account".into(),
                    operation_key: format!("mixed-{provider}"),
                    hubu_authorization_id: format!("token-{provider}"),
                    hubu_claim_id: Some(format!("claim-{provider}")),
                    hubu_token_reference: HubuTokenReference::new(format!("token-{provider}"))
                        .unwrap(),
                    authorized_minor: amount,
                    authorization_currency: "USD".into(),
                    normalized_input: if is_flux {
                        json!({"prompt":"draw a cat","image_count":1,"image_size":"1k"})
                    } else {
                        json!({"prompt":"draw a cat","image_count":1})
                    },
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
                    pricing_snapshot,
                    pricing_schema_version: i64::from(snapshot.schema_version),
                    execution_scope: None,
                    created_at: "now".into(),
                })
                .unwrap()
        };
        let gemini = create(
            "google",
            "gemini_developer_image",
            "gemini-3.1-flash-lite-image",
            "google-v1",
            25,
        );
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
        let secrets = Arc::new(Secrets::default());
        let runner = provider_execution_runner(
            repository.clone(),
            hubu.clone(),
            artifacts.clone(),
            providers,
            secrets.clone(),
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
        assert_eq!(counts.get("gemini_developer_image"), Some(&1));
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
        let resolved = secrets.0.lock().unwrap();
        for expected in [
            ("gongbu.google", "gemini"),
            ("gongbu.ideogram", "ideogram"),
            ("gongbu.flux", "flux"),
        ] {
            assert_eq!(
                resolved
                    .iter()
                    .filter(|reference| { reference.0 == expected.0 && reference.1 == expected.1 })
                    .count(),
                2
            );
        }
        assert_eq!(resolved.len(), 6);
        drop(resolved);
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
        let gemini_attempt = repository
            .get_provider_attempt_for_execution(&gemini.execution_id)
            .unwrap();
        let flux_attempt = repository
            .get_provider_attempt_for_execution(&flux.execution_id)
            .unwrap();
        assert_eq!(gemini_attempt.provider, "google");
        assert_eq!(flux_attempt.provider, "flux");
        assert_ne!(
            gemini_attempt.provider_attempt_id,
            flux_attempt.provider_attempt_id
        );
        assert_ne!(
            repository
                .get_execution(&gemini.execution_id)
                .unwrap()
                .provider_config_digest,
            repository
                .get_execution(&flux.execution_id)
                .unwrap()
                .provider_config_digest
        );
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
        let recovered_legacy_flux = repository.get_execution(&flux.execution_id).unwrap();
        assert!(recovered_legacy_flux
            .normalized_input
            .get("options")
            .is_none());
        assert!(recovered_legacy_flux
            .pricing_snapshot
            .get("output_dimensions")
            .is_none());
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
            actual_vendor_cost: Some(crate::provider_contract::ActualVendorCost {
                amount: 10,
                scale: 19,
                currency: "USD".into(),
            }),
            provider_request_id: Some("request-2".into()),
            provider_operation_id: Some("operation-2".into()),
            artifacts: vec![crate::provider_contract::NormalizedArtifact {
                media_type: "image/png".into(),
                bytes: vec![1],
            }],
        };
        assert!(matches!(
            normalize_provider_success(invalid_success),
            Err(WorkflowActivityError::Ambiguous(code)) if code == "invalid_provider_success"
        ));
    }

    #[test]
    fn invalid_async_submission_evidence_is_discarded_and_local_poll_failures_reconcile() {
        let unsafe_submission = AdapterSubmission::Pending(AsyncProviderOperation {
            provider_request_id: Some("https://storage.invalid/raw?signature=secret".into()),
            provider_operation_id: "https://provider.invalid/operation/1".into(),
            polling_host: "api.bfl.ai".into(),
            deadline_unix_ms: 1_800_000_000_000,
        });
        assert!(matches!(
            normalize_provider_submission(unsafe_submission),
            Err(WorkflowActivityError::Ambiguous(code)) if code == "invalid_provider_operation"
        ));

        let checkpoint = AsyncProviderOperation {
            provider_request_id: Some("request-170".into()),
            provider_operation_id: "operation-170".into(),
            polling_host: "api.bfl.ai".into(),
            deadline_unix_ms: 1_800_000_000_000,
        };
        assert_eq!(
            reconcile_local_poll_failure(
                WorkflowActivityError::Proven("secret_unavailable".into()),
                &checkpoint,
            ),
            WorkflowActivityError::AmbiguousWithEvidence {
                code: "secret_unavailable".into(),
                request_id: Some("request-170".into()),
                operation_id: Some("operation-170".into()),
            }
        );
    }
}
