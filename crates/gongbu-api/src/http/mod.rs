//! Versioned authenticated Execution and Artifact HTTP contract.
//!
//! Transport adapters authenticate an installation-scoped service caller and pass
//! the method/path/body here. Execution identity comes only from Hubu authorization.
use crate::{
    artifacts::{ArtifactService, Error as ArtifactError},
    attestation::{AttestationError, RedactionAttestor},
    execution_scope::{for_target, ExecutionScope},
    hubu::{HttpClientError, SpendAuthorizationResolver},
    persistence::{
        Artifact, CreateExecutionParams, Error as PersistenceError, Execution,
        HubuAuthorizationSnapshot, HubuTokenReference, Repository,
    },
    provider::{
        contract::{ContractError, NormalizedRequest, OutputDimensions, PricingSnapshot},
        flux2_api,
        registry::{ExecutionTarget, ValidatedProviderCatalog},
    },
    provider_targets::{Error as TargetError, ProviderConfigVersion, TargetKey},
    temporal::{ExecutionScheduler, ScheduleError},
    workflow::{OperatorReconciliationRequest, ReconciliationAction},
};
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub use gongbu_build_info::API_SCHEMA_VERSION as SCHEMA_VERSION;
pub const V1_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthenticatedCaller(());

impl AuthenticatedCaller {
    /// Construct only after validating the installation's caller capability.
    pub fn service_installation() -> Self {
        Self(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateExecutionV2Request {
    pub schema_version: u32,
    pub spend_auth_token_id: String,
    pub input: Value,
    pub input_schema_version: i64,
    pub target_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ExecutionTargetCatalogResponse {
    pub schema_version: u32,
    pub targets: Vec<ExecutionTarget>,
}

#[derive(Clone, Debug)]
struct CreateExecutionRequest {
    spend_auth_token_id: String,
    operation_key: Option<String>,
    hubu_claim_id: Option<String>,
    authorization: Option<Money>,
    execution_scope: Option<ExecutionScope>,
    input: Value,
    input_schema_version: i64,
    workload_type: String,
    provider: String,
    adapter: String,
    model: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Money {
    pub amount_minor: i64,
    pub currency: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReconciliationRequest {
    pub schema_version: u32,
    pub action_id: String,
    pub action: ReconciliationAction,
    pub evidence: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ExecutionResponse {
    pub schema_version: u32,
    pub execution_id: String,
    pub operation_key: String,
    pub status: ExecutionStatus,
    pub outcome: Option<String>,
    pub failure: Option<FailureResponse>,
    pub authorization: Money,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub timing: ExecutionTimingResponse,
    pub provider_transport: ProviderTransportResponse,
}

/// Agent-safe durable counts of provider-boundary transport calls.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProviderTransportResponse {
    pub schema_version: u32,
    pub poll_count: u64,
    pub artifact_fetch_count: u64,
}

/// Agent-safe elapsed time derived from Gongbu-owned durable timestamps.
///
/// The provider interval starts immediately before request transmission and
/// ends when Gongbu durably records the provider result. It remains unavailable
/// until both boundaries exist; raw provider-attempt identifiers and timestamps
/// are intentionally not projected through the public execution contract.
/// The execution interval spans durable admission through terminal completion;
/// non-provider time is emitted only when checked subtraction of those two
/// intervals is valid.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ExecutionTimingResponse {
    pub schema_version: u32,
    pub scope: String,
    pub execution_total_ms: Option<u64>,
    pub provider_interaction_ms: Option<u64>,
    pub non_provider_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Pending,
    Preflighting,
    Claimed,
    Executing,
    Settling,
    Succeeded,
    Released,
    Failed,
    ReconciliationRequired,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct FailureResponse {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ArtifactListResponse {
    pub schema_version: u32,
    pub execution_id: String,
    pub artifacts: Vec<ArtifactResponse>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ArtifactResponse {
    pub artifact_id: String,
    pub execution_id: String,
    pub kind: String,
    pub media_type: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub metadata: Value,
    pub metadata_schema_version: i64,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ProviderCatalogResponse {
    pub schema_version: u32,
    pub contracts: Vec<crate::provider::provider_contracts::CatalogContract>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ErrorResponse {
    pub schema_version: u32,
    pub error: ErrorBody,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct ApiError {
    status: u16,
    code: &'static str,
    message: &'static str,
    reason_code: Option<&'static str>,
    fields: Option<&'static [&'static str]>,
}

impl ApiError {
    fn new(status: u16, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
            reason_code: None,
            fields: None,
        }
    }
    fn unauthorized() -> Self {
        Self::new(401, "unauthorized", "authentication is required")
    }
    fn validation() -> Self {
        Self::new(400, "invalid_request", "request validation failed")
    }
    fn validation_with_diagnostics(
        reason_code: &'static str,
        fields: &'static [&'static str],
    ) -> Self {
        Self {
            reason_code: Some(reason_code),
            fields: Some(fields),
            ..Self::validation()
        }
    }
    fn not_found() -> Self {
        Self::new(404, "not_found", "resource not found")
    }
    fn conflict() -> Self {
        Self::new(
            409,
            "immutable_scope_conflict",
            "operation key was already used with different immutable input",
        )
    }
    fn attestation_not_ready() -> Self {
        Self::new(
            409,
            "attestation_not_ready",
            "execution is not ready for redaction attestation",
        )
    }
    fn not_ready() -> Self {
        Self::new(
            503,
            "not_ready",
            "execution admission is temporarily unavailable",
        )
    }
    fn internal() -> Self {
        Self::new(500, "internal_error", "request could not be completed")
    }
    fn response(&self, schema_version: u32) -> HttpResponse {
        json_response(
            self.status,
            &ErrorResponse {
                schema_version,
                error: ErrorBody {
                    code: self.code.into(),
                    message: self.message.into(),
                    reason_code: self.reason_code.map(str::to_owned),
                    fields: self
                        .fields
                        .map(|fields| fields.iter().map(|field| (*field).to_owned()).collect()),
                },
            },
        )
    }
}

#[derive(Clone)]
pub struct Api {
    repository: Repository,
    artifacts: ArtifactService,
    providers: ValidatedProviderCatalog,
    scheduler: Arc<dyn ExecutionScheduler>,
    now: Arc<dyn Fn() -> String + Send + Sync>,
    maximum_spend_minor: i64,
    authorization_resolver: Option<Arc<dyn SpendAuthorizationResolver + Send + Sync>>,
    redaction_attestor: Option<RedactionAttestor>,
}

impl Api {
    pub fn new(
        repository: Repository,
        artifacts: ArtifactService,
        providers: ValidatedProviderCatalog,
        scheduler: Arc<dyn ExecutionScheduler>,
        now: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self::new_with_maximum_spend(repository, artifacts, providers, scheduler, i64::MAX, now)
    }

    pub fn new_with_maximum_spend(
        repository: Repository,
        artifacts: ArtifactService,
        providers: ValidatedProviderCatalog,
        scheduler: Arc<dyn ExecutionScheduler>,
        maximum_spend_minor: i64,
        now: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            repository,
            artifacts,
            providers,
            scheduler,
            now: Arc::new(now),
            maximum_spend_minor,
            authorization_resolver: None,
            redaction_attestor: None,
        }
    }

    pub fn new_with_authorization_resolver(
        repository: Repository,
        artifacts: ArtifactService,
        providers: ValidatedProviderCatalog,
        scheduler: Arc<dyn ExecutionScheduler>,
        maximum_spend_minor: i64,
        authorization_resolver: Arc<dyn SpendAuthorizationResolver + Send + Sync>,
        now: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            repository,
            artifacts,
            providers,
            scheduler,
            now: Arc::new(now),
            maximum_spend_minor,
            authorization_resolver: Some(authorization_resolver),
            redaction_attestor: None,
        }
    }

    /// Enable the production, execution-bound redaction attestation path with
    /// the same Gongbu-owned credential provider used by execution activities.
    pub fn with_redaction_attestation_secrets(
        mut self,
        secrets: Arc<dyn crate::secrets::SecretProvider>,
    ) -> Self {
        self.redaction_attestor = Some(RedactionAttestor::new(
            self.repository.clone(),
            self.artifacts.clone(),
            self.providers.clone(),
            secrets,
        ));
        self
    }

    pub fn handle(
        &self,
        method: &str,
        path: &str,
        caller: Option<&AuthenticatedCaller>,
        body: &[u8],
    ) -> HttpResponse {
        let result = caller
            .ok_or_else(ApiError::unauthorized)
            .and_then(|caller| {
                let segments: Vec<_> = path.trim_matches('/').split('/').collect();
                match (method, segments.as_slice()) {
                    ("POST", ["v2", "executions"]) => self.create_v2(caller, body),
                    ("GET", ["v2", "execution-targets"]) => self.list_execution_targets(caller),
                    ("GET", ["v1", "executions", execution_id]) => {
                        self.get_execution(caller, execution_id)
                    }
                    ("POST", ["v1", "executions", execution_id, "reconciliation"]) => {
                        self.reconcile(caller, execution_id, body)
                    }
                    ("GET", ["v1", "executions", execution_id, "artifacts"]) => {
                        self.list_artifacts(caller, execution_id)
                    }
                    ("GET", ["v1", "executions", execution_id, "redaction-attestation"])
                        if body.is_empty() =>
                    {
                        self.redaction_attestation(caller, execution_id)
                    }
                    ("GET", ["v1", "executions", _, "redaction-attestation"]) => {
                        Err(ApiError::validation())
                    }
                    ("GET", ["v1", "artifacts", artifact_id]) => {
                        self.get_artifact(caller, artifact_id)
                    }
                    ("GET", ["v1", "provider-catalog"]) => self.provider_catalog(caller),
                    _ => Err(ApiError::not_found()),
                }
            });
        let response_schema_version = if path.starts_with("/v1/") {
            V1_SCHEMA_VERSION
        } else {
            SCHEMA_VERSION
        };
        result.unwrap_or_else(|error| error.response(response_schema_version))
    }

    fn provider_catalog(&self, _caller: &AuthenticatedCaller) -> Result<HttpResponse, ApiError> {
        Ok(json_response(
            200,
            &ProviderCatalogResponse {
                schema_version: 1,
                contracts: self.providers.provider_contracts().to_vec(),
            },
        ))
    }

    fn create_v2(
        &self,
        caller: &AuthenticatedCaller,
        body: &[u8],
    ) -> Result<HttpResponse, ApiError> {
        let request: CreateExecutionV2Request =
            serde_json::from_slice(body).map_err(|_| ApiError::validation())?;
        if request.schema_version != SCHEMA_VERSION {
            return Err(ApiError::validation());
        }
        let target = resolve_v2_target(&request, &self.providers, &self.repository)?;
        let request = translate_v2(request, target)?;
        self.create(caller, request, SCHEMA_VERSION)
    }

    fn list_execution_targets(
        &self,
        _caller: &AuthenticatedCaller,
    ) -> Result<HttpResponse, ApiError> {
        Ok(json_response(
            200,
            &ExecutionTargetCatalogResponse {
                schema_version: SCHEMA_VERSION,
                targets: self.providers.execution_targets(),
            },
        ))
    }

    fn create(
        &self,
        _caller: &AuthenticatedCaller,
        request: CreateExecutionRequest,
        response_schema_version: u32,
    ) -> Result<HttpResponse, ApiError> {
        validate_create(&request)?;
        let spend_auth_token_id = request.spend_auth_token_id.clone();
        let submitted_input = canonicalize(&request.input);
        let existing = match self
            .repository
            .get_execution_by_spend_auth_token(&spend_auth_token_id)
        {
            Ok(existing) => Some(existing),
            Err(PersistenceError::NotFound) => None,
            Err(error) => return Err(map_persistence(error)),
        };
        if let Some(existing) = existing.as_ref() {
            if immutable_request_matches(existing, &request, &submitted_input) {
                if existing.status == "pending" {
                    self.scheduler
                        .schedule(&existing.execution_id)
                        .map_err(map_schedule_error)?;
                }
                return Ok(json_response(
                    200,
                    &execution_response(
                        &self.repository,
                        existing.clone(),
                        response_schema_version,
                    )?,
                ));
            }
        }
        let (normalized_input, output_dimensions) =
            normalize_provider_input(&request, submitted_input)?;
        if let Some(existing) = existing {
            if !immutable_request_matches(&existing, &request, &normalized_input) {
                return Err(ApiError::conflict());
            }
            if existing.status == "pending" {
                self.scheduler
                    .schedule(&existing.execution_id)
                    .map_err(map_schedule_error)?;
            }
            return Ok(json_response(
                200,
                &execution_response(&self.repository, existing, response_schema_version)?,
            ));
        }
        let target_key = TargetKey::new(
            &request.workload_type,
            &request.provider,
            &request.adapter,
            &request.model,
        )
        .map_err(map_target_error)?;
        let resolved = self.providers.resolve_active(&target_key).map_err(|_| {
            ApiError::validation_with_diagnostics("target_not_selectable", &["target_id"])
        })?;
        let image_count = input_quantity(&normalized_input, "image_count")?;
        if image_count.is_some_and(|count| {
            u64::try_from(count).map_or(true, |count| {
                count > self.artifacts.max_artifacts_per_execution()
            })
        }) {
            return Err(ApiError::validation());
        }
        let pricing_request = NormalizedRequest {
            provider: resolved.provider.clone(),
            model: resolved.model.clone(),
            image_count,
            input_tokens: input_quantity(&normalized_input, "input_tokens")?,
            max_output_tokens: input_quantity(&normalized_input, "max_output_tokens")?,
            image_size: normalized_input
                .get("image_size")
                .and_then(|value| value.as_str())
                .map(str::to_owned),
            output_dimensions,
        };
        let pricing_snapshot = self
            .providers
            .pricing()
            .snapshot_for_target(&resolved.target_key(), &pricing_request)
            .map_err(map_pricing_error)?;
        if pricing_snapshot.estimated_amount_minor > self.maximum_spend_minor {
            return Err(ApiError::validation());
        }
        // Tokenization belongs to provider-bound request normalization, which is
        // not part of this HTTP/persistence milestone. Fail closed rather than
        // accepting a caller-asserted token count that could underfund execution.
        if !pricing_snapshot.is_image_only() {
            return Err(ApiError::validation());
        }
        let authorization = self
            .authorization_resolver
            .as_ref()
            .ok_or_else(ApiError::internal)?
            .resolve_authorization(&spend_auth_token_id)
            .map_err(map_hubu_resolution)?;
        let execution_scope =
            for_target(&resolved.provider, &resolved.adapter).ok_or_else(ApiError::validation)?;
        let created_at = (self.now)();
        let authorization_expires_at = DateTime::parse_from_rfc3339(&authorization.expires_at)
            .map_err(|_| ApiError::validation())?;
        let admission_time =
            DateTime::parse_from_rfc3339(&created_at).map_err(|_| ApiError::internal())?;
        // Hubu's consumed, frozen, and remaining hold fields are the aggregate
        // balance of the containing budget. Only the individual hold status
        // and amount are valid execution-admission invariants.
        if authorization.spend_auth_token_id != spend_auth_token_id
            || authorization.status != "available"
            || authorization_expires_at <= admission_time
            || authorization.budget_hold.status != "frozen"
            || authorization.budget_hold.amount_cents != authorization.amount_cents
            || authorization.account_id.trim().is_empty()
            || authorization.operation_key.trim().is_empty()
            || authorization.decision_id.trim().is_empty()
            || authorization.amount_cents != pricing_snapshot.estimated_amount_minor
            || !authorization
                .currency
                .eq_ignore_ascii_case(&pricing_snapshot.currency)
            || authorization.execution_scope.as_ref() != Some(&execution_scope)
            || !legacy_authorization_matches(
                &request,
                &authorization,
                &execution_scope,
                &pricing_snapshot,
            )
        {
            return Err(ApiError::validation());
        }
        let input_hash = immutable_hash(
            &request,
            &authorization,
            resolved,
            &pricing_snapshot,
            &normalized_input,
        )?;
        let authorization_snapshot = HubuAuthorizationSnapshot {
            account_id: authorization.account_id.clone(),
            agent_id: authorization.agent_id.clone(),
            operation_key: authorization.operation_key.clone(),
            decision_id: authorization.decision_id.clone(),
            spend_auth_token_id: authorization.spend_auth_token_id.clone(),
            amount_minor: authorization.amount_cents,
            currency: authorization.currency.to_ascii_uppercase(),
            execution_scope: execution_scope.clone(),
            lease_profile: authorization.lease_profile.clone(),
            expires_at: authorization.expires_at.clone(),
            authorization_status: authorization.status.clone(),
            task_id: authorization.task_id.clone(),
            reason: authorization.reason.clone(),
        };
        let pricing_schema_version = i64::from(pricing_snapshot.schema_version);
        let params = CreateExecutionParams {
            account_id: authorization.account_id.clone(),
            operation_key: authorization.operation_key.trim().to_owned(),
            // Both legacy execution columns retain their historical token-ID
            // meaning. The authoritative decision ID exists only in the
            // separately named Hubu authorization snapshot.
            hubu_authorization_id: spend_auth_token_id.clone(),
            hubu_claim_id: None,
            hubu_token_reference: HubuTokenReference::new(spend_auth_token_id)
                .map_err(|_| ApiError::validation())?,
            authorized_minor: authorization.amount_cents,
            authorization_currency: authorization.currency.to_ascii_uppercase(),
            normalized_input,
            input_hash: input_hash.clone(),
            input_schema_version: request.input_schema_version,
            target: format!(
                "{}/{}/{}/{}",
                resolved.workload_type, resolved.provider, resolved.adapter, resolved.model
            ),
            config_version: resolved.provider_config_version.clone(),
            workload_type: resolved.workload_type.clone(),
            provider: resolved.provider.clone(),
            adapter: resolved.adapter.clone(),
            model: resolved.model.clone(),
            provider_config_version: resolved.provider_config_version.clone(),
            provider_config_digest: resolved.digest().to_owned(),
            pricing_snapshot: serde_json::to_value(pricing_snapshot)
                .map_err(|_| ApiError::internal())?,
            pricing_schema_version,
            execution_scope: Some(execution_scope),
            created_at,
        };
        let execution = self
            .repository
            .create_execution_with_authorization(&params, &authorization_snapshot)
            .map_err(map_persistence)?;
        if !immutable_params_match(&execution, &params) {
            return Err(ApiError::conflict());
        }
        self.scheduler
            .schedule(&execution.execution_id)
            .map_err(map_schedule_error)?;
        Ok(json_response(
            200,
            &execution_response(&self.repository, execution, response_schema_version)?,
        ))
    }

    fn authorized_execution(
        &self,
        _caller: &AuthenticatedCaller,
        execution_id: &str,
    ) -> Result<Execution, ApiError> {
        let execution = self
            .repository
            .get_execution(execution_id)
            .map_err(map_persistence)?;
        Ok(execution)
    }

    fn reconcile(
        &self,
        caller: &AuthenticatedCaller,
        execution_id: &str,
        body: &[u8],
    ) -> Result<HttpResponse, ApiError> {
        let execution = self.authorized_execution(caller, execution_id)?;
        if execution.status != "reconciliation_required" {
            return Err(ApiError::validation());
        }
        let request: ReconciliationRequest =
            serde_json::from_slice(body).map_err(|_| ApiError::validation())?;
        if request.schema_version != V1_SCHEMA_VERSION
            || request.action_id.trim().is_empty()
            || !request.evidence.is_object()
        {
            return Err(ApiError::validation());
        }
        self.scheduler
            .reconcile(
                execution_id,
                OperatorReconciliationRequest {
                    action_id: request.action_id,
                    action: request.action,
                    evidence: request.evidence,
                },
            )
            .map_err(|_| ApiError::internal())?;
        Ok(json_response(
            202,
            &execution_response(&self.repository, execution, V1_SCHEMA_VERSION)?,
        ))
    }

    fn get_execution(
        &self,
        caller: &AuthenticatedCaller,
        execution_id: &str,
    ) -> Result<HttpResponse, ApiError> {
        Ok(json_response(
            200,
            &execution_response(
                &self.repository,
                self.authorized_execution(caller, execution_id)?,
                V1_SCHEMA_VERSION,
            )?,
        ))
    }

    fn list_artifacts(
        &self,
        caller: &AuthenticatedCaller,
        execution_id: &str,
    ) -> Result<HttpResponse, ApiError> {
        let execution = self.authorized_execution(caller, execution_id)?;
        let artifacts = self
            .artifacts
            .list_for_account(execution_id, &execution.account_id)
            .map_err(map_artifact_error)?;
        Ok(json_response(
            200,
            &ArtifactListResponse {
                schema_version: V1_SCHEMA_VERSION,
                execution_id: execution_id.into(),
                artifacts: artifacts.into_iter().map(artifact_response).collect(),
            },
        ))
    }

    fn redaction_attestation(
        &self,
        caller: &AuthenticatedCaller,
        execution_id: &str,
    ) -> Result<HttpResponse, ApiError> {
        let attestor = self
            .redaction_attestor
            .as_ref()
            .ok_or_else(ApiError::internal)?;
        let execution = self.authorized_execution(caller, execution_id)?;
        let artifacts = self
            .artifacts
            .list_for_account(execution_id, &execution.account_id)
            .map_err(map_artifact_error)?;
        let public_execution =
            execution_response(&self.repository, execution.clone(), V1_SCHEMA_VERSION)?;
        // Scan a fixed allowlist of the public execution projection. The
        // general execution response intentionally contains the private,
        // caller-bound operation key; it must never become secret-comparison
        // candidate material for this attestation.
        let public_execution = json!({
            "schema_version": public_execution.schema_version,
            "execution_id": public_execution.execution_id,
            "status": public_execution.status,
            "outcome": public_execution.outcome,
            "failure": public_execution.failure,
            "authorization": public_execution.authorization,
            "created_at": public_execution.created_at,
            "updated_at": public_execution.updated_at,
            "started_at": public_execution.started_at,
            "completed_at": public_execution.completed_at,
            "timing": public_execution.timing,
            "provider_transport": public_execution.provider_transport,
        });
        let public_artifacts = serde_json::to_value(ArtifactListResponse {
            schema_version: V1_SCHEMA_VERSION,
            execution_id: execution_id.into(),
            artifacts: artifacts.into_iter().map(artifact_response).collect(),
        })
        .map_err(|_| ApiError::internal())?;
        let public_catalog = serde_json::to_value(ProviderCatalogResponse {
            schema_version: V1_SCHEMA_VERSION,
            contracts: self.providers.provider_contracts().to_vec(),
        })
        .map_err(|_| ApiError::internal())?;
        let attestation = attestor
            .attest(
                &execution,
                &[public_execution, public_artifacts, public_catalog],
            )
            .map_err(map_attestation_error)?;
        Ok(json_response(200, &attestation))
    }

    fn get_artifact(
        &self,
        caller: &AuthenticatedCaller,
        artifact_id: &str,
    ) -> Result<HttpResponse, ApiError> {
        let artifact = self
            .repository
            .get_artifact(artifact_id)
            .map_err(map_persistence)?;
        let execution = self.authorized_execution(caller, &artifact.execution_id)?;
        let retrieved = self
            .artifacts
            .retrieve_for_account(artifact_id, &execution.account_id)
            .map_err(map_artifact_error)?;
        Ok(HttpResponse {
            status: 200,
            content_type: retrieved.artifact.media_type,
            body: retrieved.bytes,
        })
    }
}

fn normalize_provider_input(
    request: &CreateExecutionRequest,
    mut input: Value,
) -> Result<(Value, Option<OutputDimensions>), ApiError> {
    let output_dimensions = if request.provider == flux2_api::PROVIDER_ID
        && request.adapter == flux2_api::ADAPTER_ID
        && request.model == flux2_api::MODEL_ID
    {
        Some(flux2_api::bind_output_dimensions(&mut input).map_err(|_| ApiError::validation())?)
    } else {
        None
    };
    Ok((canonicalize(&input), output_dimensions))
}

fn input_quantity(input: &Value, field: &str) -> Result<Option<i64>, ApiError> {
    match input.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .filter(|quantity| *quantity >= 0)
            .map(Some)
            .ok_or_else(ApiError::validation),
    }
}

fn immutable_request_matches(
    execution: &Execution,
    request: &CreateExecutionRequest,
    normalized_input: &Value,
) -> bool {
    request
        .operation_key
        .as_ref()
        .is_none_or(|value| execution.operation_key == value.trim())
        && request
            .hubu_claim_id
            .as_ref()
            .is_none_or(|value| execution.hubu_claim_id.as_deref() == Some(value.trim()))
        && request.authorization.as_ref().is_none_or(|money| {
            execution.authorized_minor == money.amount_minor
                && execution
                    .authorization_currency
                    .eq_ignore_ascii_case(&money.currency)
        })
        && request
            .execution_scope
            .as_ref()
            .is_none_or(|scope| execution.execution_scope.as_ref() == Some(scope))
        && &execution.normalized_input == normalized_input
        && execution.input_schema_version == request.input_schema_version
        && execution.workload_type == request.workload_type
        && execution.provider == request.provider
        && execution.adapter == request.adapter
        && execution.model == request.model
}

fn immutable_params_match(execution: &Execution, params: &CreateExecutionParams) -> bool {
    execution.hubu_authorization_id == params.hubu_authorization_id
        && execution.hubu_claim_id == params.hubu_claim_id
        && execution.hubu_token_reference == params.hubu_token_reference
        && execution.authorized_minor == params.authorized_minor
        && execution
            .authorization_currency
            .eq_ignore_ascii_case(&params.authorization_currency)
        && execution.normalized_input == params.normalized_input
        && execution.input_schema_version == params.input_schema_version
        && execution.workload_type == params.workload_type
        && execution.provider == params.provider
        && execution.adapter == params.adapter
        && execution.model == params.model
}

fn resolve_v2_target(
    request: &CreateExecutionV2Request,
    providers: &ValidatedProviderCatalog,
    repository: &Repository,
) -> Result<TargetKey, ApiError> {
    let target_id = request.target_id.as_str();
    match repository.get_execution_by_spend_auth_token(&request.spend_auth_token_id) {
        Ok(existing) => {
            let persisted = TargetKey::new(
                existing.workload_type,
                existing.provider,
                existing.adapter,
                existing.model,
            )
            .map_err(map_target_error)?;
            if persisted.public_id() == target_id {
                return Ok(persisted);
            }
        }
        Err(PersistenceError::NotFound) => {}
        Err(error) => return Err(map_persistence(error)),
    }
    providers
        .resolve_target_id(target_id)
        .map(ProviderConfigVersion::target_key)
        .map_err(|_| ApiError::validation_with_diagnostics("target_not_selectable", &["target_id"]))
}

fn translate_v2(
    request: CreateExecutionV2Request,
    target: TargetKey,
) -> Result<CreateExecutionRequest, ApiError> {
    if request.schema_version != SCHEMA_VERSION {
        return Err(ApiError::validation());
    }
    Ok(CreateExecutionRequest {
        spend_auth_token_id: request.spend_auth_token_id,
        operation_key: None,
        hubu_claim_id: None,
        authorization: None,
        execution_scope: None,
        input: request.input,
        input_schema_version: request.input_schema_version,
        workload_type: target.workload_type,
        provider: target.provider,
        adapter: target.adapter,
        model: target.model,
    })
}

fn validate_create(request: &CreateExecutionRequest) -> Result<(), ApiError> {
    let spend_auth_token_id = request.spend_auth_token_id.trim();
    if spend_auth_token_id.is_empty()
        || spend_auth_token_id.len() > 255
        || request.input_schema_version < 1
        || !request.input.is_object()
        || [
            request.operation_key.as_deref(),
            request.hubu_claim_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|value| value.trim().is_empty() || value.len() > 255)
        || request.authorization.as_ref().is_some_and(|money| {
            money.amount_minor < 0
                || money.currency.len() != 3
                || !money
                    .currency
                    .bytes()
                    .all(|byte| byte.is_ascii_alphabetic())
        })
        || [
            &request.workload_type,
            &request.provider,
            &request.adapter,
            &request.model,
        ]
        .iter()
        .any(|value| value.trim().is_empty() || value.len() > 255)
    {
        return Err(ApiError::validation());
    }
    HubuTokenReference::new(spend_auth_token_id).map_err(|_| ApiError::validation())?;
    Ok(())
}

fn legacy_authorization_matches(
    request: &CreateExecutionRequest,
    authorization: &crate::hubu::ExecutorSpendResponse,
    execution_scope: &ExecutionScope,
    pricing_snapshot: &PricingSnapshot,
) -> bool {
    request
        .operation_key
        .as_ref()
        .is_none_or(|value| authorization.operation_key == value.trim())
        && request.hubu_claim_id.is_none()
        && request.authorization.as_ref().is_none_or(|money| {
            money.amount_minor == authorization.amount_cents
                && money.amount_minor == pricing_snapshot.estimated_amount_minor
                && money.currency.eq_ignore_ascii_case(&authorization.currency)
                && money
                    .currency
                    .eq_ignore_ascii_case(&pricing_snapshot.currency)
        })
        && request
            .execution_scope
            .as_ref()
            .is_none_or(|scope| scope == execution_scope)
}

fn immutable_hash(
    request: &CreateExecutionRequest,
    authorization: &crate::hubu::ExecutorSpendResponse,
    resolved: &ProviderConfigVersion,
    pricing_snapshot: &PricingSnapshot,
    normalized_input: &Value,
) -> Result<String, ApiError> {
    let scope = json!({
        "operation_key": authorization.operation_key,
        "decision_id": authorization.decision_id,
        "spend_auth_token_id": authorization.spend_auth_token_id,
        "authorization": {
            "amount_minor": authorization.amount_cents,
            "currency": authorization.currency,
        },
        "task_id": authorization.task_id,
        "reason": authorization.reason,
        "lease_profile": authorization.lease_profile,
        "expires_at": authorization.expires_at,
        "input": normalized_input,
        "input_schema_version": request.input_schema_version,
        "workload_type": resolved.workload_type,
        "provider": resolved.provider,
        "execution_scope": for_target(&resolved.provider, &resolved.adapter),
        "adapter": resolved.adapter,
        "model": resolved.model,
        "provider_config_version": resolved.provider_config_version,
        "provider_config_digest": resolved.digest(),
        "pricing_snapshot": pricing_snapshot,
        "pricing_schema_version": pricing_snapshot.schema_version,
    });
    let bytes = serde_json::to_vec(&canonicalize(&scope)).map_err(|_| ApiError::validation())?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries: Vec<_> = object.iter().collect();
            entries.sort_by_key(|(key, _)| *key);
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key.clone(), canonicalize(value)))
                    .collect::<Map<_, _>>(),
            )
        }
        Value::Array(values) => Value::Array(values.iter().map(canonicalize).collect()),
        _ => value.clone(),
    }
}

fn execution_response(
    repository: &Repository,
    execution: Execution,
    schema_version: u32,
) -> Result<ExecutionResponse, ApiError> {
    let status = match execution.status.as_str() {
        "pending" => ExecutionStatus::Pending,
        "preflighting" => ExecutionStatus::Preflighting,
        "claimed" => ExecutionStatus::Claimed,
        "executing" => ExecutionStatus::Executing,
        "settling" => ExecutionStatus::Settling,
        "succeeded" => ExecutionStatus::Succeeded,
        "failed" => ExecutionStatus::Failed,
        "released" => ExecutionStatus::Released,
        "reconciliation_required" => ExecutionStatus::ReconciliationRequired,
        _ => return Err(ApiError::internal()),
    };
    let failure = execution.failure_code.map(|code| FailureResponse {
        code,
        message: execution
            .failure_message_redacted
            .unwrap_or_else(|| "execution failed".into()),
    });
    let execution_total_ms = elapsed_ms(
        Some(execution.created_at.as_str()),
        execution.completed_at.as_deref(),
    );
    let provider_attempt =
        match repository.get_provider_attempt_for_execution(&execution.execution_id) {
            Ok(attempt) => Some(attempt),
            Err(PersistenceError::NotFound) => None,
            Err(error) => return Err(map_persistence(error)),
        };
    let provider_interaction_ms = provider_attempt.as_ref().and_then(|attempt| {
        elapsed_ms(
            attempt.transmission_started_at.as_deref(),
            attempt.completed_at.as_deref(),
        )
    });
    let timing = ExecutionTimingResponse {
        schema_version: 1,
        scope: "gongbu_execution".into(),
        execution_total_ms,
        provider_interaction_ms,
        non_provider_ms: execution_total_ms
            .zip(provider_interaction_ms)
            .and_then(|(total, provider)| total.checked_sub(provider)),
    };
    Ok(ExecutionResponse {
        schema_version,
        execution_id: execution.execution_id,
        operation_key: execution.operation_key,
        status,
        outcome: execution.outcome,
        failure,
        authorization: Money {
            amount_minor: execution.authorized_minor,
            currency: execution.authorization_currency,
        },
        created_at: execution.created_at,
        updated_at: execution.updated_at,
        started_at: execution.started_at,
        completed_at: execution.completed_at,
        timing,
        provider_transport: ProviderTransportResponse {
            schema_version: 1,
            poll_count: provider_attempt
                .as_ref()
                .map_or(0, |attempt| attempt.provider_poll_count),
            artifact_fetch_count: provider_attempt
                .as_ref()
                .map_or(0, |attempt| attempt.artifact_fetch_count),
        },
    })
}

fn elapsed_ms(started_at: Option<&str>, completed_at: Option<&str>) -> Option<u64> {
    let started_at = DateTime::parse_from_rfc3339(started_at?).ok()?;
    let completed_at = DateTime::parse_from_rfc3339(completed_at?).ok()?;
    u64::try_from(
        completed_at
            .signed_duration_since(started_at)
            .num_milliseconds(),
    )
    .ok()
}

fn artifact_response(artifact: Artifact) -> ArtifactResponse {
    ArtifactResponse {
        artifact_id: artifact.artifact_id,
        execution_id: artifact.execution_id,
        kind: artifact.kind,
        media_type: artifact.media_type,
        size_bytes: artifact.size_bytes,
        sha256: artifact.sha256,
        metadata: artifact.metadata,
        metadata_schema_version: artifact.metadata_schema_version,
        created_at: artifact.created_at,
    }
}

fn map_persistence(error: PersistenceError) -> ApiError {
    match error {
        PersistenceError::NotFound => ApiError::not_found(),
        PersistenceError::Invalid(_) | PersistenceError::OverAuthorization => {
            ApiError::validation()
        }
        _ => ApiError::internal(),
    }
}

fn map_schedule_error(error: ScheduleError) -> ApiError {
    match error {
        ScheduleError::Unavailable => ApiError::not_ready(),
    }
}

fn map_attestation_error(error: AttestationError) -> ApiError {
    match error {
        AttestationError::NotFound => ApiError::not_found(),
        AttestationError::NotReady => ApiError::attestation_not_ready(),
        AttestationError::UnsupportedTarget => ApiError::validation(),
        AttestationError::SecretUnavailable | AttestationError::Internal => ApiError::internal(),
    }
}

fn map_hubu_resolution(error: HttpClientError) -> ApiError {
    match error {
        HttpClientError::Status { status, .. } if (400..500).contains(&status) => {
            ApiError::validation()
        }
        _ => ApiError::internal(),
    }
}

fn map_artifact_error(error: ArtifactError) -> ApiError {
    match error {
        ArtifactError::Persistence(PersistenceError::NotFound) => ApiError::not_found(),
        _ => ApiError::internal(),
    }
}

fn map_target_error(error: TargetError) -> ApiError {
    match error {
        TargetError::NotSelectable
        | TargetError::ExecutionDisabled
        | TargetError::NotConfigured
        | TargetError::DigestMismatch => ApiError::validation(),
        _ => ApiError::internal(),
    }
}

fn map_pricing_error(error: ContractError) -> ApiError {
    match error {
        ContractError::UnsupportedTarget => ApiError::validation_with_diagnostics(
            "pricing_selector_not_matched",
            &["input.image_size"],
        ),
        ContractError::IndeterminableCost | ContractError::InsufficientAuthorization => {
            ApiError::validation()
        }
        _ => ApiError::internal(),
    }
}

fn json_response(status: u16, body: &impl Serialize) -> HttpResponse {
    HttpResponse {
        status,
        content_type: "application/json".into(),
        body: serde_json::to_vec(body).expect("response schemas must serialize"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        artifacts::{ArtifactLimits, LocalFsStorage},
        execution::{AttemptResult, CreateReceiptParams, ExecutionUpdate, LifecycleOutcome},
        provider::{
            contract::{
                ActualVendorCost, AdapterCapabilities, AdapterOutcome, AsyncProviderOperation,
                NormalizedUsage, PricingCatalog, ProviderAdapter, ProviderFailure,
            },
            registry::ProviderRegistry,
        },
        provider_targets::ProviderTargetConfig,
        secrets::{ProviderSecret, SecretError, SecretProvider, SecretReference},
    };
    use image::{DynamicImage, ImageOutputFormat, RgbaImage};
    use serde_json::json;
    use std::{
        io::Cursor,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Barrier,
        },
        thread,
    };
    use tempfile::TempDir;

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
            unreachable!("HTTP admission does not invoke providers")
        }
    }

    struct AttestationSecrets(&'static str);

    impl SecretProvider for AttestationSecrets {
        fn resolve(&self, reference: &SecretReference) -> Result<ProviderSecret, SecretError> {
            assert_eq!(reference.service(), "gongbu.bfl.hubu-hub-172");
            assert_eq!(reference.account(), "pikachu-live-qualification-v1");
            Ok(crate::secrets::secret_for_test(self.0))
        }
    }

    fn catalog(targets: ProviderTargetConfig, pricing: PricingCatalog) -> ValidatedProviderCatalog {
        let mut registry = ProviderRegistry::new();
        registry.register("example", "fixture", |_| Ok(Arc::new(AdmissionAdapter)));
        ValidatedProviderCatalog::bind(targets, pricing, &registry).unwrap()
    }

    #[derive(Default)]
    struct Scheduler(std::sync::Mutex<Vec<String>>, AtomicBool);
    impl ExecutionScheduler for Scheduler {
        fn schedule(&self, execution_id: &str) -> Result<(), ScheduleError> {
            if self.1.load(Ordering::SeqCst) {
                return Err(ScheduleError::Unavailable);
            }
            self.0.lock().unwrap().push(execution_id.into());
            Ok(())
        }
        fn reconcile(
            &self,
            execution_id: &str,
            _: OperatorReconciliationRequest,
        ) -> Result<(), ScheduleError> {
            self.0
                .lock()
                .unwrap()
                .push(format!("reconcile:{execution_id}"));
            Ok(())
        }
    }

    struct Fixture {
        api: Api,
        repository: Repository,
        artifacts: ArtifactService,
        caller: AuthenticatedCaller,
        scheduler: Arc<Scheduler>,
        resolver: Arc<TestResolver>,
        _root: TempDir,
    }

    #[derive(Default)]
    struct TestResolver {
        calls: AtomicUsize,
    }
    impl SpendAuthorizationResolver for TestResolver {
        fn resolve_authorization(
            &self,
            spend_auth_token_id: &str,
        ) -> Result<crate::hubu::ExecutorSpendResponse, HttpClientError> {
            let previous_calls = self.calls.fetch_add(1, Ordering::SeqCst);
            if spend_auth_token_id.starts_with("consumed-legacy-replay") && previous_calls > 0 {
                return Err(HttpClientError::Status {
                    status: 409,
                    body: "authorization already consumed".into(),
                });
            }
            let mut response = crate::hubu::ExecutorSpendResponse {
                operation_key: spend_auth_token_id.into(),
                reason: "test execution".into(),
                spend_auth_token_id: spend_auth_token_id.into(),
                decision_id: format!("decision-{spend_auth_token_id}"),
                account_id: "account-a".into(),
                agent_id: "agent-a".into(),
                amount_cents: 100,
                currency: "USD".into(),
                merchant: None,
                execution_scope: for_target("example", "fixture"),
                task_id: Some("linear:HUB-72".into()),
                lease_profile: "default".into(),
                status: "available".into(),
                expires_at: "2026-08-05T21:00:00Z".into(),
                budget_hold: crate::hubu::BudgetHold {
                    hold_id: "hold".into(),
                    budget_id: "budget".into(),
                    status: "frozen".into(),
                    amount_cents: 100,
                    consumed_amount_cents: 0,
                    frozen_amount_cents: 100,
                    remaining_amount_cents: 0,
                },
            };
            if spend_auth_token_id.starts_with("price-mismatch") {
                response.amount_cents += 1;
            } else if spend_auth_token_id.starts_with("identity-mismatch") {
                response.account_id = "account-b".into();
            } else if spend_auth_token_id.starts_with("scope-mismatch") {
                response.execution_scope = for_target("google", "gemini_developer_image");
            } else if spend_auth_token_id.starts_with("token-swap") {
                response.spend_auth_token_id = "different-token".into();
            } else if spend_auth_token_id.starts_with("expired") {
                response.expires_at = "2026-08-05T19:00:00Z".into();
            } else if spend_auth_token_id.starts_with("aggregate-consumed") {
                response.budget_hold.consumed_amount_cents = 100;
                response.budget_hold.remaining_amount_cents = 800;
            } else if spend_auth_token_id.starts_with("aggregate-frozen") {
                response.budget_hold.frozen_amount_cents = 200;
                response.budget_hold.remaining_amount_cents = 800;
            } else if spend_auth_token_id.starts_with("hold-status-mismatch") {
                response.budget_hold.status = "settled".into();
            } else if spend_auth_token_id.starts_with("hold-amount-mismatch") {
                response.budget_hold.amount_cents += 1;
            }
            if spend_auth_token_id.starts_with("flux-") {
                response.execution_scope = for_target("flux", "flux2_api");
                let amount = if spend_auth_token_id.contains("4k") {
                    8
                } else if spend_auth_token_id.contains("2k")
                    || spend_auth_token_id == "flux-restart-replay"
                {
                    5
                } else {
                    3
                };
                response.amount_cents = amount;
                response.budget_hold.amount_cents = amount;
                response.budget_hold.frozen_amount_cents = amount;
            }
            if spend_auth_token_id.starts_with("flux-managed-") {
                response.amount_cents = 3;
                response.budget_hold.amount_cents = 3;
                response.budget_hold.frozen_amount_cents = 3;
                response.expires_at = "2026-08-28T21:00:00Z".into();
            }
            if spend_auth_token_id == "flux-managed-hub-172-attestation" {
                response.operation_key = "codex:v1:11111111111111111111111111111111".into();
                response.reason = "HUB-172 guarded FLUX live qualification: one 1k PNG.".into();
                response.account_id = "aga_n063sdm0pepd".into();
                response.agent_id = "agt_wk3q33h3j6w8".into();
                response.task_id = None;
            }
            Ok(response)
        }
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let repository = Repository::open(
            root.path().join("gongbu.sqlite3"),
            crate::redaction::Redactor::default(),
        )
        .unwrap();
        let artifacts = ArtifactService::new(
            repository.clone(),
            LocalFsStorage::new(root.path()),
            ArtifactLimits::default(),
        );
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
        targets.validate().unwrap();
        let pricing = PricingCatalog::from_json(
            br#"{
                "schema_version":2,
                "catalog_version":"prices-v2",
                "rules":[{
                    "rule_id":"example-image",
                    "provider":"example",
                    "model":"image-v1",
                    "currency":"USD",
                    "components":[{
                        "unit":"image",
                        "rate_numerator_minor":100,
                        "rate_denominator":1
                    }]
                }]
            }"#,
        )
        .unwrap();
        let scheduler = Arc::new(Scheduler::default());
        let resolver = Arc::new(TestResolver::default());
        Fixture {
            api: Api::new_with_authorization_resolver(
                repository.clone(),
                artifacts.clone(),
                catalog(targets, pricing),
                scheduler.clone(),
                i64::MAX,
                resolver.clone(),
                || "2026-08-05T20:00:00Z".into(),
            ),
            repository,
            artifacts,
            caller: AuthenticatedCaller::service_installation(),
            scheduler,
            resolver,
            _root: root,
        }
    }

    fn flux_catalog(catalog_version: &str) -> ValidatedProviderCatalog {
        let _ = catalog_version;
        supported_flux_catalog()
    }

    fn supported_flux_catalog() -> ValidatedProviderCatalog {
        let document: Value = serde_json::from_str(include_str!(
            "../../../../contracts/provider-contracts-v1.json"
        ))
        .unwrap();
        let contract_definition = document["contracts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|contract| contract["contract"] == "hubu.flux-2-pro.text-to-image/v1")
            .unwrap();
        let mut target = contract_definition["target"].clone();
        target["secret_service"] = json!("gongbu.bfl.hubu-hub-172");
        target["secret_account"] = json!("pikachu-live-qualification-v1");
        let policies = &contract_definition["policies"];
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
            "provider_configs": [target]
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
        let mut catalog = ValidatedProviderCatalog::bind(
            targets,
            pricing,
            &ProviderRegistry::production(&ArtifactLimits::default()),
        )
        .unwrap();
        catalog.mark_credential_references_present();
        catalog
    }

    fn supported_shipped_catalog() -> ValidatedProviderCatalog {
        let document: Value = serde_json::from_str(include_str!(
            "../../../../contracts/provider-contracts-v1.json"
        ))
        .unwrap();
        let contracts = document["contracts"].as_array().unwrap();
        let bindings = contracts
            .iter()
            .map(|contract| {
                let policies = &contract["policies"];
                json!({
                    "contract":contract["contract"],
                    "pricing_version":contract["pricing_version"],
                    "poll_policy":policies["poll"],
                    "artifact_delivery_policy":policies["artifact_delivery"],
                    "recovery_policy":policies["recovery"],
                    "generation_retries":policies["generation_retries"],
                    "fallback":policies["fallback"]
                })
            })
            .collect::<Vec<_>>();
        let provider_configs = contracts
            .iter()
            .map(|contract| {
                let mut target = contract["target"].clone();
                let google = target["provider"] == "google";
                target["secret_service"] = json!(if google {
                    "operator.google"
                } else {
                    "operator.bfl"
                });
                target["secret_account"] = json!(if google { "gemini" } else { "flux" });
                target
            })
            .collect::<Vec<_>>();
        let rules = contracts
            .iter()
            .flat_map(|contract| contract["pricing_rules"].as_array().unwrap().clone())
            .collect::<Vec<_>>();
        let targets: ProviderTargetConfig = serde_json::from_value(json!({
            "schema_version":3,
            "contract_bindings":bindings,
            "provider_configs":provider_configs
        }))
        .unwrap();
        let pricing = PricingCatalog::from_json(
            &serde_json::to_vec(&json!({
                "schema_version":2,
                "catalog_version":"operator-shipped-composite-2026-09-01-v1",
                "rules":rules
            }))
            .unwrap(),
        )
        .unwrap();
        let mut catalog = ValidatedProviderCatalog::bind(
            targets,
            pricing,
            &ProviderRegistry::production(&ArtifactLimits::default()),
        )
        .unwrap();
        catalog.mark_credential_references_present();
        catalog
    }

    fn flux_request(operation_key: &str, preset: Option<&str>, options: Option<Value>) -> Value {
        let mut input = json!({"prompt":"cat","image_count":1});
        if let Some(preset) = preset {
            input["image_size"] = json!(preset);
        }
        if let Some(options) = options {
            input["options"] = options;
        }
        json!({
            "schema_version":2,
            "spend_auth_token_id":operation_key,
            "input":input,
            "input_schema_version":1,
            "target_id":TargetKey::new("image_generation", "flux", "flux2_api", "flux-2-pro")
                .unwrap()
                .public_id()
        })
    }

    fn flux_api(fixture: &Fixture, catalog_version: &str) -> Api {
        Api::new_with_authorization_resolver(
            fixture.repository.clone(),
            fixture.artifacts.clone(),
            flux_catalog(catalog_version),
            fixture.scheduler.clone(),
            i64::MAX,
            fixture.resolver.clone(),
            || "2026-08-05T20:00:00Z".into(),
        )
    }

    fn successful_hub_172_attestation_execution(
        fixture: &Fixture,
        secret: &'static str,
        provider_request_id: &str,
    ) -> (Api, String) {
        let api = supported_flux_attestation_api(fixture, secret);
        let mut request = flux_request(
            "flux-managed-hub-172-attestation",
            Some("1k"),
            Some(json!({"output_format":"png"})),
        );
        request["input"]["prompt"] =
            json!("A small blue circle centered on a plain white background.");
        let created = execution(&api.handle(
            "POST",
            "/v2/executions",
            Some(&fixture.caller),
            &serde_json::to_vec(&request).unwrap(),
        ));
        let preflighting = fixture
            .repository
            .update_execution(
                &created.execution_id,
                0,
                &ExecutionUpdate {
                    status: "preflighting".into(),
                    outcome: None,
                    started_at: None,
                    completed_at: None,
                    failure_code: None,
                    failure_message_redacted: None,
                    provider_outcome: None,
                    artifact_outcome: None,
                    settlement_outcome: None,
                },
                "2026-08-28T20:00:01Z",
            )
            .unwrap();
        let claimed = fixture
            .repository
            .set_claim(
                &created.execution_id,
                preflighting.version,
                "hub-172-claim",
                "2026-08-28T20:00:02Z",
            )
            .unwrap();
        let attempt = fixture
            .repository
            .start_provider_attempt(&claimed, "2026-08-28T20:00:03Z")
            .unwrap();
        fixture
            .repository
            .begin_provider_transmission(&attempt.provider_attempt_id, "2026-08-28T20:00:04Z")
            .unwrap();
        fixture
            .repository
            .record_provider_operation(
                &attempt.provider_attempt_id,
                &AsyncProviderOperation {
                    provider_request_id: Some(provider_request_id.into()),
                    provider_operation_id: "hub-172-provider-operation".into(),
                    polling_host: "api.bfl.ai".into(),
                    deadline_unix_ms: 1_788_000_000_000,
                },
                "2026-08-28T20:00:05Z",
            )
            .unwrap();
        fixture
            .repository
            .record_provider_poll(&attempt.provider_attempt_id)
            .unwrap();
        fixture
            .repository
            .record_artifact_fetch(&attempt.provider_attempt_id)
            .unwrap();
        let vendor_cost = ActualVendorCost::new(3, 2, "USD").unwrap();
        fixture
            .repository
            .complete_provider_attempt(
                &attempt.provider_attempt_id,
                &AttemptResult {
                    outcome: "succeeded".into(),
                    completed_at: "2026-08-28T20:00:06Z".into(),
                    usage: serde_json::to_value(NormalizedUsage {
                        images: Some(1),
                        input_tokens: None,
                        output_tokens: None,
                    })
                    .unwrap(),
                    usage_schema_version: 1,
                    actual_vendor_cost: Some(vendor_cost.clone()),
                    failure_code: None,
                    failure_message_redacted: None,
                    provider_request_id: Some(provider_request_id.into()),
                    provider_operation_id: Some("hub-172-provider-operation".into()),
                },
            )
            .unwrap();
        let mut png = Vec::new();
        DynamicImage::ImageRgba8(RgbaImage::new(1024, 1024))
            .write_to(&mut Cursor::new(&mut png), ImageOutputFormat::Png)
            .unwrap();
        fixture
            .artifacts
            .store_image(
                &created.execution_id,
                Some(&attempt.provider_attempt_id),
                "image/png",
                &png,
                "2026-08-28T20:00:07Z",
            )
            .unwrap();
        let executing = fixture
            .repository
            .get_execution(&created.execution_id)
            .unwrap();
        let settling = fixture
            .repository
            .complete_artifact_persistence(
                &executing,
                &attempt.provider_attempt_id,
                "2026-08-28T20:00:08Z",
            )
            .unwrap();
        let receipt = fixture
            .repository
            .create_receipt(&CreateReceiptParams {
                receipt_id: "hub-172-receipt".into(),
                execution_id: created.execution_id.clone(),
                provider_attempt_id: attempt.provider_attempt_id,
                settlement_minor: 3,
                currency: "USD".into(),
                pricing_catalog_version: "bfl-flux-2-pro-usd-2026-08-28-v1".into(),
                actual_vendor_cost: vendor_cost,
                created_at: "2026-08-28T20:00:09Z".into(),
                settled_at: None,
                hubu_settlement_id: None,
            })
            .unwrap();
        fixture
            .repository
            .begin_settlement_transmission(&receipt.receipt_id, "2026-08-28T20:00:10Z")
            .unwrap();
        fixture
            .repository
            .complete_receipt(
                &receipt.receipt_id,
                "hub-172-settlement",
                "2026-08-28T20:00:11Z",
            )
            .unwrap();
        fixture
            .repository
            .update_execution(
                &created.execution_id,
                settling.version,
                &ExecutionUpdate {
                    status: "succeeded".into(),
                    outcome: Some("succeeded".into()),
                    started_at: Some("2026-08-28T20:00:03Z".into()),
                    completed_at: Some("2026-08-28T20:00:12Z".into()),
                    failure_code: None,
                    failure_message_redacted: None,
                    provider_outcome: Some(LifecycleOutcome::Succeeded),
                    artifact_outcome: Some(LifecycleOutcome::Succeeded),
                    settlement_outcome: Some(LifecycleOutcome::Succeeded),
                },
                "2026-08-28T20:00:12Z",
            )
            .unwrap();
        (api, created.execution_id)
    }

    fn supported_flux_attestation_api(fixture: &Fixture, secret: &'static str) -> Api {
        Api::new_with_authorization_resolver(
            fixture.repository.clone(),
            fixture.artifacts.clone(),
            supported_flux_catalog(),
            fixture.scheduler.clone(),
            3,
            fixture.resolver.clone(),
            || "2026-08-28T20:00:00Z".into(),
        )
        .with_redaction_attestation_secrets(Arc::new(AttestationSecrets(secret)))
    }

    #[test]
    fn terminal_flux_redaction_attestation_is_strict_and_secret_free() {
        const CANARY: &str = "fixture-provider-secret-attestation-canary-9f83";
        let fixture = fixture();
        let (api, execution_id) =
            successful_hub_172_attestation_execution(&fixture, CANARY, "hub-172-provider-request");

        let response = api.handle(
            "GET",
            &format!("/v1/executions/{execution_id}/redaction-attestation"),
            Some(&fixture.caller),
            &[],
        );
        assert_eq!(response.status, 200);
        let body = String::from_utf8(response.body).unwrap();
        assert!(!body.contains(CANARY));
        assert!(!body.contains(&execution_id));
        assert!(!body.contains("https://"));
        assert!(!body.contains("storage_key"));
        let attestation: crate::attestation::RedactionAttestation =
            serde_json::from_str(&body).unwrap();
        assert_eq!(
            attestation.attestation_contract,
            "gongbu.flux-redaction-attestation/v1"
        );
        assert!(attestation.allowlist_projection);
        assert!(attestation.terminal_execution);
        assert!(attestation.registered_provider_secret_resolved);
        assert!(attestation.registered_provider_secret_absent_from_scanned_projections);
        assert_eq!(attestation.scan.logical_database_record_count, 4);
        assert_eq!(attestation.scan.artifact_metadata_record_count, 1);
        assert_eq!(attestation.scan.public_projection_count, 3);
        assert_eq!(attestation.facts.authorization_snapshot_count, 1);
        assert_eq!(attestation.facts.claim_reference_count, 1);
        assert_eq!(attestation.facts.provider_attempt_count, 1);
        assert_eq!(attestation.facts.provider_submission_count, 1);
        assert_eq!(attestation.facts.durable_checkpoint_count, 1);
        assert_eq!(attestation.facts.provider_poll_count, 1);
        assert_eq!(attestation.facts.artifact_fetch_count, 1);
        assert_eq!(attestation.facts.artifact_count, 1);
        assert_eq!(attestation.facts.receipt_count, 1);
        assert_eq!(attestation.facts.settlement_delivery_count, 1);
        assert_eq!(attestation.facts.authorized_minor, 3);
        assert_eq!(attestation.facts.authorization_currency, "USD");
        assert_eq!(attestation.facts.provider_cost_minor, Some(3));
        assert_eq!(
            attestation.facts.provider_cost_currency.as_deref(),
            Some("USD")
        );
        assert_eq!(attestation.facts.settled_minor, Some(3));
        assert_eq!(attestation.facts.settled_currency.as_deref(), Some("USD"));
        for digest in [
            &attestation.execution_sha256,
            &attestation.artifact_sha256,
            &attestation.settlement_sha256,
            &attestation.combined_projection_sha256,
            &attestation.facts.artifact_content_sha256,
        ] {
            assert_eq!(digest.len(), 71);
            assert!(digest
                .strip_prefix("sha256:")
                .is_some_and(|value| value.bytes().all(|byte| byte.is_ascii_hexdigit())));
        }
    }

    #[test]
    fn caller_bound_operation_key_is_not_secret_probe_material() {
        const OPERATION_KEY: &str = "codex:v1:11111111111111111111111111111111";
        let fixture = fixture();
        let (api, execution_id) = successful_hub_172_attestation_execution(
            &fixture,
            OPERATION_KEY,
            "hub-172-provider-request",
        );
        let response = api.handle(
            "GET",
            &format!("/v1/executions/{execution_id}/redaction-attestation"),
            Some(&fixture.caller),
            &[],
        );
        assert_eq!(response.status, 200);
        let body = String::from_utf8(response.body).unwrap();
        assert!(!body.contains(OPERATION_KEY));
        let attestation: crate::attestation::RedactionAttestation =
            serde_json::from_str(&body).unwrap();
        assert!(attestation.registered_provider_secret_absent_from_scanned_projections);
    }

    #[test]
    fn registered_secret_detection_fails_closed_without_echoing_the_canary() {
        const CANARY: &str = "fixture-provider-secret-attestation-canary-9f83";
        let fixture = fixture();
        let (api, execution_id) =
            successful_hub_172_attestation_execution(&fixture, CANARY, CANARY);
        let response = api.handle(
            "GET",
            &format!("/v1/executions/{execution_id}/redaction-attestation"),
            Some(&fixture.caller),
            &[],
        );
        assert_eq!(response.status, 500);
        let body = String::from_utf8(response.body).unwrap();
        assert!(!body.contains(CANARY));
        assert!(!body.contains(&execution_id));
        assert!(!body.contains("secret"));
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap()["error"]["code"],
            "internal_error"
        );
    }

    #[test]
    fn redaction_attestation_rejects_body_and_nonterminal_execution() {
        const CANARY: &str = "fixture-provider-secret-attestation-canary-9f83";
        let fixture = fixture();
        let api = supported_flux_attestation_api(&fixture, CANARY);
        let created = execution(
            &api.handle(
                "POST",
                "/v2/executions",
                Some(&fixture.caller),
                &serde_json::to_vec(&flux_request(
                    "flux-managed-not-ready",
                    Some("1k"),
                    Some(json!({"output_format":"png"})),
                ))
                .unwrap(),
            ),
        );
        let path = format!(
            "/v1/executions/{}/redaction-attestation",
            created.execution_id
        );
        assert_eq!(
            api.handle("GET", &path, Some(&fixture.caller), b"candidate")
                .status,
            400
        );
        assert_eq!(
            api.handle("GET", &path, Some(&fixture.caller), &[]).status,
            409
        );

        let arbitrary = flux_api(&fixture, "prices-v2")
            .with_redaction_attestation_secrets(Arc::new(AttestationSecrets(CANARY)));
        let created = execution(
            &arbitrary.handle(
                "POST",
                "/v2/executions",
                Some(&fixture.caller),
                &serde_json::to_vec(&flux_request("flux-arbitrary-contract", Some("1k"), None))
                    .unwrap(),
            ),
        );
        fixture
            .repository
            .update_execution(
                &created.execution_id,
                0,
                &ExecutionUpdate {
                    status: "failed".into(),
                    outcome: Some("failed".into()),
                    started_at: None,
                    completed_at: Some("2026-08-05T20:00:01Z".into()),
                    failure_code: Some("fixture_failure".into()),
                    failure_message_redacted: Some("safe".into()),
                    provider_outcome: None,
                    artifact_outcome: None,
                    settlement_outcome: None,
                },
                "2026-08-05T20:00:01Z",
            )
            .unwrap();
        assert_eq!(
            arbitrary
                .handle(
                    "GET",
                    &format!(
                        "/v1/executions/{}/redaction-attestation",
                        created.execution_id
                    ),
                    Some(&fixture.caller),
                    &[],
                )
                .status,
            409
        );
    }

    #[test]
    fn operator_maximum_spend_rejects_authorization_and_price_before_persistence() {
        let fixture = fixture();
        let api = Api::new_with_authorization_resolver(
            fixture.repository.clone(),
            fixture.artifacts.clone(),
            fixture.api.providers.clone(),
            fixture.scheduler.clone(),
            99,
            fixture.resolver.clone(),
            || "2026-08-05T20:00:00Z".into(),
        );
        let response = api.handle(
            "POST",
            "/v2/executions",
            Some(&fixture.caller),
            &serde_json::to_vec(&request("over-ceiling")).unwrap(),
        );
        assert_eq!(response.status, 400);
        assert!(fixture
            .repository
            .get_execution_by_operation("account-a", "over-ceiling")
            .is_err());
    }

    #[test]
    fn managed_flux_spend_ceiling_rejects_before_authorization_or_persistence() {
        let fixture = fixture();
        let control_token = "flux-managed-at-ceiling";
        let control = Api::new_with_authorization_resolver(
            fixture.repository.clone(),
            fixture.artifacts.clone(),
            supported_flux_catalog(),
            fixture.scheduler.clone(),
            3,
            fixture.resolver.clone(),
            || "2026-08-28T20:00:00Z".into(),
        );
        let control_response = control.handle(
            "POST",
            "/v2/executions",
            Some(&fixture.caller),
            &serde_json::to_vec(&flux_request(control_token, Some("1k"), None)).unwrap(),
        );
        assert_eq!(control_response.status, 200);
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), 1);
        assert!(fixture
            .repository
            .get_execution_by_spend_auth_token(control_token)
            .is_ok());
        assert_eq!(fixture.scheduler.0.lock().unwrap().len(), 1);

        let token = "flux-managed-over-ceiling";
        let api = Api::new_with_authorization_resolver(
            fixture.repository.clone(),
            fixture.artifacts.clone(),
            supported_flux_catalog(),
            fixture.scheduler.clone(),
            2,
            fixture.resolver.clone(),
            || "2026-08-28T20:00:00Z".into(),
        );

        let response = api.handle(
            "POST",
            "/v2/executions",
            Some(&fixture.caller),
            &serde_json::to_vec(&flux_request(token, Some("1k"), None)).unwrap(),
        );

        assert_eq!(response.status, 400);
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), 1);
        assert!(fixture
            .repository
            .get_execution_by_spend_auth_token(token)
            .is_err());
        assert_eq!(fixture.scheduler.0.lock().unwrap().len(), 1);
    }

    #[test]
    fn supported_provider_catalog_is_authenticated_sanitized_and_non_live() {
        let fixture = fixture();
        let api = Api::new_with_authorization_resolver(
            fixture.repository.clone(),
            fixture.artifacts.clone(),
            supported_shipped_catalog(),
            fixture.scheduler.clone(),
            i64::MAX,
            fixture.resolver.clone(),
            || "2026-08-28T20:00:00Z".into(),
        );
        assert_eq!(
            api.handle("GET", "/v1/provider-catalog", None, b"").status,
            401
        );
        let response = api.handle("GET", "/v1/provider-catalog", Some(&fixture.caller), b"");
        assert_eq!(response.status, 200);
        let value: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(value["schema_version"], 1);
        let contracts = value["contracts"].as_array().unwrap();
        let ids = contracts
            .iter()
            .map(|contract| contract["contract"].as_str().unwrap())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            ids,
            std::collections::BTreeSet::from([
                "hubu.flux-2-pro.text-to-image/v1",
                "hubu.gemini-3.1-flash-image.text-to-image/v1",
                "hubu.gemini-3.1-flash-lite-image.text-to-image/v1"
            ])
        );
        let gemini_full = contracts
            .iter()
            .find(|contract| contract["target"]["model"] == "gemini-3.1-flash-image")
            .unwrap();
        assert_eq!(gemini_full["capability"]["presets"][2]["width"], 4096);
        assert!(contracts.iter().all(|contract| contract["readiness"]
            == json!({
                "configured":true,"credential_reference_present":true,
                "production_validated":true,"live_qualified":false,
                "live_qualification":"not_performed"
            })));
        let serialized = serde_json::to_string(&value).unwrap();
        assert!(!serialized.contains("secret_service"));
        assert!(!serialized.contains("secret_account"));
        assert!(!serialized.contains("gongbu.bfl"));
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), 0);
        assert!(fixture.scheduler.0.lock().unwrap().is_empty());
    }

    fn request(operation_key: &str) -> Value {
        json!({
            "schema_version": 2,
            "spend_auth_token_id": operation_key,
            "input": {
                "prompt": "cat",
                "image_count": 1,
                "options": {"height": 512, "width": 512}
            },
            "input_schema_version": 1,
            "target_id": TargetKey::new("image_generation", "example", "fixture", "image-v1")
                .unwrap()
                .public_id()
        })
    }

    fn legacy_request(operation_key: &str) -> Value {
        let mut request = request(operation_key);
        request["schema_version"] = json!(1);
        let token = request
            .as_object_mut()
            .unwrap()
            .remove("spend_auth_token_id")
            .unwrap();
        request["hubu_token_reference"] = token.clone();
        request["hubu_authorization_id"] = token;
        request["operation_key"] = json!(operation_key);
        request["hubu_claim_id"] = Value::Null;
        request["authorization"] = json!({"amount_minor": 100, "currency": "USD"});
        request["execution_scope"] =
            serde_json::to_value(for_target("example", "fixture").unwrap()).unwrap();
        request
    }

    #[test]
    fn canonical_create_fixture_matches_v2_schema() {
        let request: CreateExecutionV2Request = serde_json::from_str(include_str!(
            "../../../../fixtures/gongbu-create-execution-v2.json"
        ))
        .unwrap();
        assert_eq!(request.schema_version, 2);
        assert_eq!(
            request.spend_auth_token_id,
            "00000000-0000-4000-8000-000000000123"
        );
    }

    #[test]
    fn retired_v1_create_route_rejects_without_side_effects() {
        let raw = include_str!("../../../../fixtures/gongbu-create-execution-v1.json");
        let fixture = fixture();
        let response = fixture.api.handle(
            "POST",
            "/v1/executions",
            Some(&fixture.caller),
            raw.as_bytes(),
        );
        assert_eq!(response.status, 404);
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), 0);
        assert!(fixture.scheduler.0.lock().unwrap().is_empty());
    }

    fn call_create(fixture: &Fixture, request: &Value) -> HttpResponse {
        fixture.api.handle(
            "POST",
            "/v2/executions",
            Some(&fixture.caller),
            &serde_json::to_vec(request).unwrap(),
        )
    }

    fn call_create_v1(fixture: &Fixture, request: &Value) -> HttpResponse {
        fixture.api.handle(
            "POST",
            "/v1/executions",
            Some(&fixture.caller),
            &serde_json::to_vec(request).unwrap(),
        )
    }

    #[test]
    fn execution_targets_are_sanitized_and_target_id_creates_the_same_execution() {
        let fixture = fixture();
        let catalog =
            fixture
                .api
                .handle("GET", "/v2/execution-targets", Some(&fixture.caller), &[]);
        assert_eq!(catalog.status, 200);
        let catalog: Value = serde_json::from_slice(&catalog.body).unwrap();
        assert_eq!(catalog["schema_version"], 2);
        assert_eq!(catalog["targets"].as_array().unwrap().len(), 1);
        let target = &catalog["targets"][0];
        let target_id = target["target_id"].as_str().unwrap();
        assert!(target_id.starts_with("gongbu:target:v1:"));
        assert_eq!(target["workload_type"], "image_generation");
        assert_eq!(target["provider"], "example");
        assert_eq!(target["model"], "image-v1");
        assert_eq!(
            target["execution_scope"]["billing_merchant"],
            "merchant:local"
        );
        assert!(target.get("adapter").is_none());
        let serialized = target.to_string();
        for private in ["secret", "credential", "endpoint", "headers", "fixture-v1"] {
            assert!(!serialized.contains(private), "leaked {private}");
        }

        let mut selected = request("target-id-selection");
        selected["target_id"] = json!(target_id);
        let created = call_create(&fixture, &selected);
        assert_eq!(created.status, 200);
        let stored = fixture
            .repository
            .get_execution_by_operation("account-a", "target-id-selection")
            .unwrap();
        assert_eq!(stored.provider, "example");
        assert_eq!(stored.adapter, "fixture");
        assert_eq!(stored.model, "image-v1");
    }

    #[test]
    fn target_id_replay_recovers_persisted_execution_after_target_deactivation() {
        let fixture = fixture();
        let target_id = fixture.api.providers.execution_targets()[0]
            .target_id
            .clone();
        let mut selected = request("target-id-deactivation-replay");
        selected["target_id"] = json!(target_id);
        let created = execution(&call_create(&fixture, &selected));

        let inactive_targets: ProviderTargetConfig = serde_json::from_value(json!({
            "schema_version":2,
            "provider_configs":[{
                "provider_config_version":"provider-v1",
                "workload_type":"image_generation",
                "provider":"example",
                "adapter":"fixture",
                "model":"image-v1",
                "secret_service":"gongbu.example",
                "secret_account":"local",
                "active":false,
                "execution_enabled":true,
                "settings":{"type":"fixture"}
            }]
        }))
        .unwrap();
        inactive_targets.validate().unwrap();
        let replay_api = Api::new_with_authorization_resolver(
            fixture.repository.clone(),
            fixture.artifacts.clone(),
            catalog(inactive_targets, fixture.api.providers.pricing().clone()),
            fixture.scheduler.clone(),
            i64::MAX,
            fixture.resolver.clone(),
            || "2026-08-05T20:00:00Z".into(),
        );
        assert!(replay_api.providers.execution_targets().is_empty());

        let replayed = replay_api.handle(
            "POST",
            "/v2/executions",
            Some(&fixture.caller),
            &serde_json::to_vec(&selected).unwrap(),
        );
        assert_eq!(replayed.status, 200);
        assert_eq!(execution(&replayed).execution_id, created.execution_id);
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn target_id_cannot_be_mixed_with_or_escape_the_operator_catalog() {
        let fixture = fixture();
        let mut mixed = request("mixed-target-selector");
        mixed["provider"] = json!("example");
        assert_eq!(call_create(&fixture, &mixed).status, 400);

        let mut unknown = request("unknown-target-id");
        unknown["target_id"] = json!(format!("gongbu:target:v1:{}", "0".repeat(64)));
        let response = call_create(&fixture, &unknown);
        assert_eq!(response.status, 400);
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(body["error"]["reason_code"], "target_not_selectable");
        assert_eq!(body["error"]["fields"], json!(["target_id"]));
    }

    fn execution(response: &HttpResponse) -> ExecutionResponse {
        serde_json::from_slice(&response.body).unwrap()
    }

    fn error(response: &HttpResponse) -> ErrorResponse {
        serde_json::from_slice(&response.body).unwrap()
    }

    #[test]
    fn create_get_and_replay_have_stable_aggregate_schema() {
        let fixture = fixture();
        let first = call_create(&fixture, &request("operation-1"));
        assert_eq!(first.status, 200);
        let first = execution(&first);
        assert_eq!(first.schema_version, 2);
        assert_eq!(first.status, ExecutionStatus::Pending);
        assert_eq!(first.authorization.amount_minor, 100);
        assert_eq!(first.timing.schema_version, 1);
        assert_eq!(first.timing.scope, "gongbu_execution");
        assert_eq!(first.timing.execution_total_ms, None);
        assert_eq!(first.timing.provider_interaction_ms, None);
        assert_eq!(first.timing.non_provider_ms, None);

        // Object member ordering is immaterial to canonical immutable input.
        let mut reordered = request("operation-1");
        reordered["input"] = json!({
            "options": {"width": 512, "height": 512},
            "image_count": 1,
            "prompt": "cat"
        });
        let replay = execution(&call_create(&fixture, &reordered));
        assert_eq!(replay.execution_id, first.execution_id);
        assert_eq!(
            fixture.scheduler.0.lock().unwrap().as_slice(),
            [first.execution_id.as_str(), first.execution_id.as_str()]
        );

        let fetched = fixture.api.handle(
            "GET",
            &format!("/v1/executions/{}", first.execution_id),
            Some(&fixture.caller),
            &[],
        );
        let fetched = execution(&fetched);
        assert_eq!(fetched.schema_version, 1);
        assert_eq!(fetched.execution_id, first.execution_id);
    }

    #[test]
    fn scheduling_unavailability_preserves_pending_execution_for_retry() {
        let fixture = fixture();
        let submitted = request("temporal-unavailable-retry");
        fixture.scheduler.1.store(true, Ordering::SeqCst);

        let first = call_create(&fixture, &submitted);
        assert_eq!(first.status, 503);
        assert_eq!(error(&first).error.code, "not_ready");
        let pending = fixture
            .repository
            .get_execution_by_spend_auth_token("temporal-unavailable-retry")
            .unwrap();
        assert_eq!(pending.status, "pending");
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), 1);
        assert!(fixture.scheduler.0.lock().unwrap().is_empty());

        let retry_while_unavailable = call_create(&fixture, &submitted);
        assert_eq!(retry_while_unavailable.status, 503);
        assert_eq!(
            fixture
                .repository
                .get_execution_by_spend_auth_token("temporal-unavailable-retry")
                .unwrap()
                .execution_id,
            pending.execution_id
        );
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), 1);

        fixture.scheduler.1.store(false, Ordering::SeqCst);
        let recovered = call_create(&fixture, &submitted);
        assert_eq!(recovered.status, 200);
        assert_eq!(execution(&recovered).execution_id, pending.execution_id);
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            fixture.scheduler.0.lock().unwrap().as_slice(),
            [pending.execution_id.as_str()]
        );
    }

    #[test]
    fn elapsed_timing_rejects_missing_malformed_and_negative_boundaries() {
        assert_eq!(
            elapsed_ms(
                Some("2026-08-05T00:00:02Z"),
                Some("2026-08-05T00:00:05.500Z"),
            ),
            Some(3_500)
        );
        assert_eq!(
            elapsed_ms(Some("not-a-timestamp"), Some("2026-08-05T00:00:05Z")),
            None
        );
        assert_eq!(
            elapsed_ms(Some("2026-08-05T00:00:05Z"), Some("2026-08-05T00:00:02Z"),),
            None
        );
        assert_eq!(elapsed_ms(None, Some("2026-08-05T00:00:05Z")), None);
    }

    #[test]
    fn execution_response_projects_provider_and_non_provider_durations() {
        let fixture = fixture();
        let created = execution(&call_create(&fixture, &request("timed-execution")));
        let pending = fixture
            .repository
            .get_execution(&created.execution_id)
            .unwrap();
        let preflighting = fixture
            .repository
            .update_execution(
                &pending.execution_id,
                pending.version,
                &ExecutionUpdate {
                    status: "preflighting".into(),
                    outcome: None,
                    started_at: Some("2026-08-05T20:00:00.100Z".into()),
                    completed_at: None,
                    failure_code: None,
                    failure_message_redacted: None,
                    provider_outcome: None,
                    artifact_outcome: None,
                    settlement_outcome: None,
                },
                "2026-08-05T20:00:00.100Z",
            )
            .unwrap();
        let claimed = fixture
            .repository
            .set_claim(
                &preflighting.execution_id,
                preflighting.version,
                "claim-timing",
                "2026-08-05T20:00:00.200Z",
            )
            .unwrap();
        let attempt = fixture
            .repository
            .start_provider_attempt(&claimed, "2026-08-05T20:00:00.300Z")
            .unwrap();
        fixture
            .repository
            .begin_provider_transmission(&attempt.provider_attempt_id, "2026-08-05T20:00:00.400Z")
            .unwrap();
        fixture
            .repository
            .record_provider_poll(&attempt.provider_attempt_id)
            .unwrap();
        fixture
            .repository
            .record_provider_poll(&attempt.provider_attempt_id)
            .unwrap();
        fixture
            .repository
            .record_artifact_fetch(&attempt.provider_attempt_id)
            .unwrap();
        fixture
            .repository
            .complete_provider_attempt(
                &attempt.provider_attempt_id,
                &AttemptResult {
                    outcome: "failed".into(),
                    completed_at: "2026-08-05T20:00:03.900Z".into(),
                    usage: json!({}),
                    usage_schema_version: 1,
                    actual_vendor_cost: None,
                    failure_code: Some("provider_rejected".into()),
                    failure_message_redacted: None,
                    provider_request_id: None,
                    provider_operation_id: None,
                },
            )
            .unwrap();
        let executing = fixture
            .repository
            .get_execution(&created.execution_id)
            .unwrap();
        fixture
            .repository
            .update_execution(
                &executing.execution_id,
                executing.version,
                &ExecutionUpdate {
                    status: "released".into(),
                    outcome: Some("failed".into()),
                    started_at: None,
                    completed_at: Some("2026-08-05T20:00:04Z".into()),
                    failure_code: None,
                    failure_message_redacted: None,
                    provider_outcome: None,
                    artifact_outcome: None,
                    settlement_outcome: None,
                },
                "2026-08-05T20:00:04Z",
            )
            .unwrap();

        let response = fixture.api.handle(
            "GET",
            &format!("/v1/executions/{}", created.execution_id),
            Some(&fixture.caller),
            &[],
        );
        let response = execution(&response);
        assert_eq!(response.timing.execution_total_ms, Some(4_000));
        assert_eq!(response.timing.provider_interaction_ms, Some(3_500));
        assert_eq!(response.timing.non_provider_ms, Some(500));
        assert_eq!(response.provider_transport.schema_version, 1);
        assert_eq!(response.provider_transport.poll_count, 2);
        assert_eq!(response.provider_transport.artifact_fetch_count, 1);
    }

    #[test]
    fn replay_with_gongbu_managed_claim_matches_after_claim_is_recorded() {
        let fixture = fixture();
        let submitted = request("managed-claim-replay");
        let created = execution(&call_create(&fixture, &submitted));
        let mut persisted = fixture
            .repository
            .get_execution(&created.execution_id)
            .unwrap();
        persisted.hubu_claim_id = Some("claim-created-by-workflow".into());
        let submitted: CreateExecutionV2Request = serde_json::from_value(submitted).unwrap();
        let target =
            resolve_v2_target(&submitted, &fixture.api.providers, &fixture.repository).unwrap();
        let decoded = translate_v2(submitted, target).unwrap();

        assert!(immutable_request_matches(
            &persisted,
            &decoded,
            &canonicalize(&decoded.input)
        ));
    }

    #[test]
    fn v1_create_route_is_retired_without_side_effects() {
        let fixture = fixture();
        let legacy = legacy_request("legacy-token");
        assert_eq!(call_create_v1(&fixture, &legacy).status, 404);
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), 0);
        assert!(fixture.scheduler.0.lock().unwrap().is_empty());
        assert!(fixture
            .repository
            .get_execution_by_hubu_token("account-a", "legacy-token")
            .is_err());
    }

    #[test]
    fn every_v1_authority_assertion_fails_closed_when_broadened() {
        let fixture = fixture();
        let cases = [
            ("operation", "/operation_key", json!("caller-operation")),
            ("amount", "/authorization/amount_minor", json!(101)),
            ("currency", "/authorization/currency", json!("EUR")),
            (
                "scope",
                "/execution_scope/provider/id",
                json!("provider:other"),
            ),
            ("claim", "/hubu_claim_id", json!("caller-claim")),
        ];
        for (name, pointer, value) in cases {
            let token = format!("legacy-{name}-mismatch");
            let mut request = legacy_request(&token);
            *request.pointer_mut(pointer).unwrap() = value;
            assert_eq!(call_create_v1(&fixture, &request).status, 404, "{name}");
            assert!(fixture
                .repository
                .get_execution_by_hubu_token("account-a", &token)
                .is_err());
        }
        assert_eq!(fixture.scheduler.0.lock().unwrap().len(), 0);
    }

    #[test]
    fn mismatched_authoritative_snapshots_are_rejected_before_persistence() {
        let fixture = fixture();
        for token in [
            "price-mismatch-1",
            "scope-mismatch-1",
            "token-swap-1",
            "expired-1",
        ] {
            assert_eq!(call_create(&fixture, &request(token)).status, 400);
            assert!(fixture
                .repository
                .get_execution_by_hubu_token("account-a", token)
                .is_err());
        }
    }

    #[test]
    fn admission_persists_two_authoritative_principals_from_hubu() {
        let fixture = fixture();
        let first = execution(&call_create(&fixture, &request("account-a-token")));
        let second = execution(&call_create(&fixture, &request("identity-mismatch-token")));

        assert_ne!(first.execution_id, second.execution_id);
        assert_eq!(first.operation_key, "account-a-token");
        assert_eq!(second.operation_key, "identity-mismatch-token");
        let first_snapshot = fixture
            .repository
            .get_hubu_authorization_snapshot(&first.execution_id)
            .unwrap();
        let second_snapshot = fixture
            .repository
            .get_hubu_authorization_snapshot(&second.execution_id)
            .unwrap();
        assert_eq!(
            (
                first_snapshot.account_id.as_str(),
                first_snapshot.agent_id.as_str()
            ),
            ("account-a", "agent-a")
        );
        assert_eq!(
            (
                second_snapshot.account_id.as_str(),
                second_snapshot.agent_id.as_str()
            ),
            ("account-b", "agent-a")
        );
    }

    #[test]
    fn aggregate_budget_balances_do_not_control_execution_admission() {
        let fixture = fixture();
        for token in ["aggregate-consumed-1", "aggregate-frozen-1"] {
            assert_eq!(
                call_create(&fixture, &request(token)).status,
                200,
                "{token}"
            );
            assert!(fixture
                .repository
                .get_execution_by_hubu_token("account-a", token)
                .is_ok());
        }
        assert_eq!(fixture.scheduler.0.lock().unwrap().len(), 2);
    }

    #[test]
    fn individual_budget_hold_invariants_remain_fail_closed() {
        let fixture = fixture();
        for token in ["hold-status-mismatch-1", "hold-amount-mismatch-1"] {
            assert_eq!(
                call_create(&fixture, &request(token)).status,
                400,
                "{token}"
            );
            assert!(fixture
                .repository
                .get_execution_by_hubu_token("account-a", token)
                .is_err());
        }
        assert_eq!(fixture.scheduler.0.lock().unwrap().len(), 0);
    }

    #[test]
    fn authoritative_snapshot_survives_repository_restart_and_token_replays() {
        let fixture = fixture();
        let created = execution(&call_create(&fixture, &request("restart-token")));
        let restarted = Repository::open(
            fixture._root.path().join("gongbu.sqlite3"),
            crate::redaction::Redactor::default(),
        )
        .unwrap();
        let snapshot = restarted
            .get_hubu_authorization_snapshot(&created.execution_id)
            .unwrap();
        assert_eq!(snapshot.operation_key, "restart-token");
        assert_eq!(snapshot.task_id.as_deref(), Some("linear:HUB-72"));
        assert_eq!(snapshot.reason, "test execution");
        assert_eq!(snapshot.lease_profile, "default");
        assert_eq!(snapshot.authorization_status, "available");
        assert_eq!(
            restarted
                .get_execution_by_hubu_token("account-a", "restart-token")
                .unwrap()
                .execution_id,
            created.execution_id
        );
    }

    #[test]
    fn historical_v1_envelope_cannot_create_after_restart() {
        let fixture = fixture();
        let submitted = legacy_request("legacy-restart-token");
        assert_eq!(call_create_v1(&fixture, &submitted).status, 404);
        let restarted_repository = Repository::open(
            fixture._root.path().join("gongbu.sqlite3"),
            crate::redaction::Redactor::default(),
        )
        .unwrap();
        let restarted = Api::new_with_authorization_resolver(
            restarted_repository,
            fixture.artifacts.clone(),
            fixture.api.providers.clone(),
            fixture.scheduler.clone(),
            i64::MAX,
            fixture.resolver.clone(),
            || "2026-08-05T20:00:00Z".into(),
        );
        let replay = restarted.handle(
            "POST",
            "/v1/executions",
            Some(&fixture.caller),
            &serde_json::to_vec(&submitted).unwrap(),
        );
        assert_eq!(replay.status, 404);
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn pre_snapshot_consumed_authorization_replays_without_resolution() {
        let fixture = fixture();
        let submitted = request("consumed-legacy-replay-token");
        let created = execution(&call_create(&fixture, &submitted));
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            fixture
                .repository
                .delete_hubu_authorization_snapshot(&created.execution_id)
                .unwrap(),
            1
        );

        let replay = call_create(&fixture, &submitted);
        assert_eq!(replay.status, 200);
        assert_eq!(execution(&replay).execution_id, created.execution_id);
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), 1);

        let mut changed = submitted;
        changed["input"] = json!({"prompt":"different","image_count":1});
        assert_eq!(call_create(&fixture, &changed).status, 409);
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn authenticated_operator_reconciliation_signals_stable_execution() {
        let fixture = fixture();
        let created: ExecutionResponse = serde_json::from_slice(
            &fixture
                .api
                .handle(
                    "POST",
                    "/v2/executions",
                    Some(&fixture.caller),
                    &serde_json::to_vec(&request("operator-signal")).unwrap(),
                )
                .body,
        )
        .unwrap();
        let pending = fixture
            .repository
            .get_execution(&created.execution_id)
            .unwrap();
        let pre = fixture
            .repository
            .update_execution(
                &pending.execution_id,
                pending.version,
                &crate::execution::ExecutionUpdate {
                    status: "preflighting".into(),
                    outcome: None,
                    started_at: Some("now".into()),
                    completed_at: None,
                    failure_code: None,
                    failure_message_redacted: None,
                    provider_outcome: None,
                    artifact_outcome: None,
                    settlement_outcome: None,
                },
                "now",
            )
            .unwrap();
        fixture
            .repository
            .update_execution(
                &pre.execution_id,
                pre.version,
                &crate::execution::ExecutionUpdate {
                    status: "reconciliation_required".into(),
                    outcome: Some("ambiguous".into()),
                    started_at: None,
                    completed_at: None,
                    failure_code: Some("ambiguous".into()),
                    failure_message_redacted: None,
                    provider_outcome: None,
                    artifact_outcome: None,
                    settlement_outcome: None,
                },
                "later",
            )
            .unwrap();
        let body=serde_json::to_vec(&json!({"schema_version":1,"action_id":"op-1","action":"reinspect","evidence":{"source":"operator"}})).unwrap();
        assert_eq!(
            fixture
                .api
                .handle(
                    "POST",
                    &format!("/v1/executions/{}/reconciliation", created.execution_id),
                    Some(&fixture.caller),
                    &body
                )
                .status,
            202
        );
        assert!(fixture
            .scheduler
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|v| v == &format!("reconcile:{}", created.execution_id)));
    }

    #[test]
    fn changed_immutable_scope_is_a_stable_conflict() {
        let fixture = fixture();
        assert_eq!(call_create(&fixture, &request("operation-1")).status, 200);
        let mut changed = request("operation-1");
        changed["input"]["prompt"] = json!("dog");
        let response = call_create(&fixture, &changed);
        assert_eq!(response.status, 409);
        let error: ErrorResponse = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(error.error.code, "immutable_scope_conflict");
    }

    #[test]
    fn authentication_validation_and_not_found_are_distinct() {
        let fixture = fixture();
        assert_eq!(
            fixture
                .api
                .handle("POST", "/v2/executions", None, b"{}")
                .status,
            401
        );
        let mut invalid = request("operation-1");
        invalid["account_id"] = json!("attacker-controlled");
        assert_eq!(call_create(&fixture, &invalid).status, 400);

        let created = execution(&call_create(&fixture, &request("operation-1")));
        assert_eq!(
            fixture
                .api
                .handle(
                    "GET",
                    &format!("/v1/executions/{}", created.execution_id),
                    Some(&fixture.caller),
                    &[],
                )
                .status,
            200
        );
        assert_eq!(
            fixture
                .api
                .handle("GET", "/v1/executions/missing", Some(&fixture.caller), &[],)
                .status,
            404
        );
    }

    #[test]
    fn operator_target_and_pricing_fields_cannot_be_fabricated() {
        let fixture = fixture();
        let mut unknown = request("operation-1");
        unknown["provider"] = json!("attacker-provider");
        assert_eq!(call_create(&fixture, &unknown).status, 400);

        let mut fabricated_version = request("operation-2");
        fabricated_version["provider_config_version"] = json!("attacker-version");
        assert_eq!(call_create(&fixture, &fabricated_version).status, 400);

        let mut fabricated_snapshot = request("operation-3");
        fabricated_snapshot["pricing_snapshot"] = json!({"components": []});
        assert_eq!(call_create(&fixture, &fabricated_snapshot).status, 400);
    }

    #[test]
    fn unavailable_target_reports_bounded_diagnostics_without_side_effects() {
        let fixture = fixture();
        let token = "unavailable-target";
        let mut unavailable = request(token);
        unavailable["target_id"] = json!(format!("gongbu:target:v1:{}", "0".repeat(64)));

        let response = call_create(&fixture, &unavailable);

        assert_eq!(response.status, 400);
        assert_eq!(
            error(&response),
            ErrorResponse {
                schema_version: SCHEMA_VERSION,
                error: ErrorBody {
                    code: "invalid_request".into(),
                    message: "request validation failed".into(),
                    reason_code: Some("target_not_selectable".into()),
                    fields: Some(["target_id"].into_iter().map(str::to_owned).collect()),
                },
            }
        );
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), 0);
        assert!(fixture
            .repository
            .get_execution_by_spend_auth_token(token)
            .is_err());
        assert!(fixture.scheduler.0.lock().unwrap().is_empty());
    }

    #[test]
    fn unmatched_pricing_selector_reports_input_field_without_side_effects() {
        let fixture = fixture();
        let pricing = PricingCatalog::from_json(
            br#"{
                "schema_version":2,
                "catalog_version":"selector-prices-v2",
                "rules":[{
                    "rule_id":"example-image-2k",
                    "provider":"example",
                    "model":"image-v1",
                    "currency":"USD",
                    "selector":{"image_size":"2k"},
                    "components":[{
                        "unit":"image",
                        "rate_numerator_minor":100,
                        "rate_denominator":1
                    }]
                }]
            }"#,
        )
        .unwrap();
        let api = Api::new_with_authorization_resolver(
            fixture.repository.clone(),
            fixture.artifacts.clone(),
            catalog(fixture.api.providers.targets().clone(), pricing),
            fixture.scheduler.clone(),
            i64::MAX,
            fixture.resolver.clone(),
            || "2026-08-05T20:00:00Z".into(),
        );
        let token = "unmatched-pricing-selector";
        let mut unmatched = request(token);
        unmatched["input"]["image_size"] = json!("4k");

        let response = api.handle(
            "POST",
            "/v2/executions",
            Some(&fixture.caller),
            &serde_json::to_vec(&unmatched).unwrap(),
        );

        assert_eq!(response.status, 400);
        assert_eq!(
            error(&response),
            ErrorResponse {
                schema_version: SCHEMA_VERSION,
                error: ErrorBody {
                    code: "invalid_request".into(),
                    message: "request validation failed".into(),
                    reason_code: Some("pricing_selector_not_matched".into()),
                    fields: Some(vec!["input.image_size".into()]),
                },
            }
        );
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), 0);
        assert!(fixture
            .repository
            .get_execution_by_spend_auth_token(token)
            .is_err());
        assert!(fixture.scheduler.0.lock().unwrap().is_empty());
    }

    #[test]
    fn flux_admission_freezes_every_certified_preset_before_authorization() {
        let fixture = fixture();
        let api = flux_api(&fixture, "flux-prices-v1");
        for (preset, width, height) in [("1k", 1024, 1024), ("2k", 1920, 1088), ("4k", 2048, 2048)]
        {
            let token = format!("flux-preset-{preset}");
            let response = api.handle(
                "POST",
                "/v2/executions",
                Some(&fixture.caller),
                &serde_json::to_vec(&flux_request(&token, Some(preset), None)).unwrap(),
            );
            assert_eq!(response.status, 200, "{preset}");
            let stored = fixture
                .repository
                .get_execution_by_spend_auth_token(&token)
                .unwrap();
            assert_eq!(stored.normalized_input["image_size"], preset);
            assert_eq!(stored.normalized_input["options"]["width"], width);
            assert_eq!(stored.normalized_input["options"]["height"], height);
            let snapshot: PricingSnapshot =
                serde_json::from_value(stored.pricing_snapshot).unwrap();
            assert_eq!(
                snapshot
                    .selector
                    .as_ref()
                    .map(|selector| selector.image_size.as_str()),
                Some(preset)
            );
            assert_eq!(
                snapshot.output_dimensions,
                Some(OutputDimensions { width, height })
            );
            assert_eq!(
                snapshot.pricing_rule_id,
                format!("bfl-flux-2-pro-{preset}-2026-08-28-v1")
            );
        }
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), 3);
        assert_eq!(fixture.scheduler.0.lock().unwrap().len(), 3);
    }

    #[test]
    fn flux_dimension_rejections_have_no_authorization_or_durable_side_effects() {
        let fixture = fixture();
        let api = flux_api(&fixture, "flux-prices-v1");
        let cases = [
            ("flux-missing-preset", None, None),
            ("flux-unsupported-preset", Some("8k"), None),
            ("flux-width-only", Some("1k"), Some(json!({"width":1024}))),
            ("flux-height-only", Some("1k"), Some(json!({"height":1024}))),
            (
                "flux-conflicting-preset",
                Some("1k"),
                Some(json!({"width":2048,"height":2048})),
            ),
            (
                "flux-arbitrary-dimensions",
                Some("1k"),
                Some(json!({"width":1024,"height":768})),
            ),
            (
                "flux-invalid-multiple",
                Some("1k"),
                Some(json!({"width":1000,"height":1024})),
            ),
            (
                "flux-pixel-overflow",
                Some("4k"),
                Some(json!({"width":2064,"height":2048})),
            ),
        ];
        for (token, preset, options) in cases {
            let response = api.handle(
                "POST",
                "/v2/executions",
                Some(&fixture.caller),
                &serde_json::to_vec(&flux_request(token, preset, options)).unwrap(),
            );
            assert_eq!(response.status, 400, "{token}");
            assert!(fixture
                .repository
                .get_execution_by_spend_auth_token(token)
                .is_err());
        }
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), 0);
        assert!(fixture.scheduler.0.lock().unwrap().is_empty());
    }

    #[test]
    fn flux_replay_survives_repository_restart_and_catalog_rotation() {
        let fixture = fixture();
        let token = "flux-restart-replay";
        let submitted = flux_request(token, Some("2k"), None);
        let created = execution(&flux_api(&fixture, "flux-prices-v1").handle(
            "POST",
            "/v2/executions",
            Some(&fixture.caller),
            &serde_json::to_vec(&submitted).unwrap(),
        ));
        let before = fixture
            .repository
            .get_execution_by_spend_auth_token(token)
            .unwrap();

        let reopened = Repository::open(
            fixture._root.path().join("gongbu.sqlite3"),
            crate::redaction::Redactor::default(),
        )
        .unwrap();
        let restarted_artifacts = ArtifactService::new(
            reopened.clone(),
            LocalFsStorage::new(fixture._root.path()),
            ArtifactLimits::default(),
        );
        let restarted = Api::new_with_authorization_resolver(
            reopened.clone(),
            restarted_artifacts,
            flux_catalog("flux-prices-v2"),
            fixture.scheduler.clone(),
            i64::MAX,
            fixture.resolver.clone(),
            || "2026-08-06T00:00:00Z".into(),
        );
        let replay = restarted.handle(
            "POST",
            "/v2/executions",
            Some(&fixture.caller),
            &serde_json::to_vec(&submitted).unwrap(),
        );
        assert_eq!(replay.status, 200);
        assert_eq!(execution(&replay).execution_id, created.execution_id);
        let after = reopened.get_execution_by_spend_auth_token(token).unwrap();
        assert_eq!(after.normalized_input, before.normalized_input);
        assert_eq!(after.pricing_snapshot, before.pricing_snapshot);
        assert_eq!(after.input_hash, before.input_hash);
        assert_eq!(after.normalized_input["image_size"], "2k");
        assert_eq!(after.normalized_input["options"]["width"], 1920);
        assert_eq!(after.normalized_input["options"]["height"], 1088);
        let snapshot: PricingSnapshot = serde_json::from_value(after.pricing_snapshot).unwrap();
        assert_eq!(snapshot.catalog_version, "bfl-flux-2-pro-usd-2026-08-28-v1");
        assert_eq!(snapshot.pricing_rule_id, "bfl-flux-2-pro-2k-2026-08-28-v1");
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn v2_rejects_every_legacy_authorization_field_before_resolution() {
        let fixture = fixture();
        for (field, value) in [
            ("hubu_authorization_id", json!("token")),
            ("hubu_token_reference", json!("token")),
            ("operation_key", json!("operation")),
            ("hubu_claim_id", Value::Null),
            (
                "authorization",
                json!({"amount_minor":100,"currency":"USD"}),
            ),
            (
                "execution_scope",
                serde_json::to_value(for_target("example", "fixture").unwrap()).unwrap(),
            ),
        ] {
            let token = format!("v2-legacy-{field}");
            let mut request = request(&token);
            request[field] = value;
            assert_eq!(call_create(&fixture, &request).status, 400, "{field}");
            assert!(fixture
                .repository
                .get_execution_by_hubu_token("account-a", &token)
                .is_err());
        }
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.scheduler.0.lock().unwrap().len(), 0);
    }

    #[test]
    fn replay_precedes_changed_operator_target_and_catalog_state() {
        let fixture = fixture();
        let created = execution(&call_create(&fixture, &request("operation-1")));
        let disabled_targets: ProviderTargetConfig = serde_json::from_value(json!({
            "provider_configs": [{
                "provider_config_version": "provider-v2",
                "workload_type": "image_generation",
                "provider": "example",
                "adapter": "fixture",
                "model": "image-v1",
                "secret_service": "gongbu.example",
                "secret_account": "local",
                "enabled": false
            }]
        }))
        .unwrap();
        let changed_pricing = PricingCatalog::from_json(
            br#"{
                "schema_version":2,
                "catalog_version":"prices-v2",
                "rules":[{
                    "rule_id":"example-image-v2",
                    "provider":"example",
                    "model":"image-v1",
                    "currency":"USD",
                    "components":[{
                        "unit":"image",
                        "rate_numerator_minor":200,
                        "rate_denominator":1
                    }]
                }]
            }"#,
        )
        .unwrap();
        let restarted = Api::new(
            fixture.repository.clone(),
            fixture.artifacts.clone(),
            catalog(disabled_targets, changed_pricing),
            fixture.scheduler.clone(),
            || "2026-08-06T00:00:00Z".into(),
        );
        let replay = restarted.handle(
            "POST",
            "/v2/executions",
            Some(&fixture.caller),
            &serde_json::to_vec(&request("operation-1")).unwrap(),
        );
        assert_eq!(replay.status, 200);
        assert_eq!(execution(&replay).execution_id, created.execution_id);
    }

    #[test]
    fn concurrent_create_replays_across_operator_snapshot_changes() {
        let fixture = fixture();
        let created = execution(&call_create(&fixture, &request("operation-race")));
        let changed_targets: ProviderTargetConfig = serde_json::from_value(json!({
            "provider_configs": [{
                "provider_config_version": "provider-v2",
                "workload_type": "image_generation",
                "provider": "example",
                "adapter": "fixture",
                "model": "image-v1",
                "secret_service": "gongbu.example",
                "secret_account": "local"
            }]
        }))
        .unwrap();
        let changed_pricing = PricingCatalog::from_json(
            br#"{
                "schema_version":2,"catalog_version":"prices-v2",
                "rules":[{"rule_id":"example-image-v2","provider":"example",
                "model":"image-v1","currency":"USD","components":[{
                "unit":"image","rate_numerator_minor":200,"rate_denominator":1}]}]
            }"#,
        )
        .unwrap();
        let changed_api = Api::new(
            fixture.repository.clone(),
            fixture.artifacts.clone(),
            catalog(changed_targets, changed_pricing),
            fixture.scheduler.clone(),
            || "2026-08-06T00:00:00Z".into(),
        );
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = [fixture.api.clone(), changed_api]
            .into_iter()
            .map(|api| {
                let barrier = barrier.clone();
                let owner = fixture.caller;
                thread::spawn(move || {
                    let body = serde_json::to_vec(&request("operation-race")).unwrap();
                    barrier.wait();
                    api.handle("POST", "/v2/executions", Some(&owner), &body)
                })
            })
            .collect();
        let responses: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert!(
            responses.iter().all(|response| response.status == 200),
            "statuses: {:?}",
            responses
                .iter()
                .map(|response| response.status)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            execution(&responses[0]).execution_id,
            execution(&responses[1]).execution_id
        );
        assert_eq!(execution(&responses[0]).execution_id, created.execution_id);
    }

    #[test]
    fn concurrent_new_admission_persists_one_token_and_snapshot() {
        let fixture = fixture();
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = [fixture.api.clone(), fixture.api.clone()]
            .into_iter()
            .map(|api| {
                let barrier = barrier.clone();
                let caller = fixture.caller;
                thread::spawn(move || {
                    let body = serde_json::to_vec(&request("new-admission-race")).unwrap();
                    barrier.wait();
                    api.handle("POST", "/v2/executions", Some(&caller), &body)
                })
            })
            .collect();
        let responses: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert!(
            responses.iter().all(|response| response.status == 200),
            "statuses: {:?}",
            responses
                .iter()
                .map(|response| response.status)
                .collect::<Vec<_>>()
        );
        let first = execution(&responses[0]);
        let second = execution(&responses[1]);
        assert_eq!(first.execution_id, second.execution_id);
        assert_eq!(
            fixture
                .repository
                .get_execution_by_spend_auth_token("new-admission-race")
                .unwrap()
                .execution_id,
            first.execution_id
        );
        assert_eq!(
            fixture
                .repository
                .get_hubu_authorization_snapshot(&first.execution_id)
                .unwrap()
                .spend_auth_token_id,
            "new-admission-race"
        );
    }

    #[test]
    fn token_priced_workloads_fail_closed_until_provider_normalization() {
        let fixture = fixture();
        let targets: ProviderTargetConfig = serde_json::from_value(json!({
            "provider_configs": [{
                "provider_config_version": "text-v1",
                "workload_type": "text_generation",
                "provider": "example",
                "adapter": "fixture",
                "model": "text-v1",
                "secret_service": "gongbu.example",
                "secret_account": "local"
            }]
        }))
        .unwrap();
        let pricing = PricingCatalog::from_json(
            br#"{
                "schema_version":2,"catalog_version":"prices-v2",
                "rules":[{"rule_id":"example-text","provider":"example",
                "model":"text-v1","currency":"USD","components":[{
                "unit":"input_token","rate_numerator_minor":1,"rate_denominator":1}]}]
            }"#,
        )
        .unwrap();
        let scheduler = fixture.scheduler.clone();
        let api = Api::new(
            fixture.repository,
            fixture.artifacts,
            catalog(targets, pricing),
            scheduler,
            || "2026-08-06T00:00:00Z".into(),
        );
        let mut text_request = request("operation-text");
        text_request["workload_type"] = json!("text_generation");
        text_request["model"] = json!("text-v1");
        text_request["input"] = json!({"prompt": "very long prompt", "input_tokens": 1});
        let response = api.handle(
            "POST",
            "/v2/executions",
            Some(&fixture.caller),
            &serde_json::to_vec(&text_request).unwrap(),
        );
        assert_eq!(response.status, 400);
    }

    #[test]
    fn mixed_image_and_caller_token_pricing_fails_closed() {
        let fixture = fixture();
        let targets: ProviderTargetConfig = serde_json::from_value(json!({
            "provider_configs": [{
                "provider_config_version": "mixed-v1",
                "workload_type": "image_generation",
                "provider": "example",
                "adapter": "fixture",
                "model": "mixed-v1",
                "secret_service": "gongbu.example",
                "secret_account": "local"
            }]
        }))
        .unwrap();
        let pricing = PricingCatalog::from_json(
            br#"{
                "schema_version":2,"catalog_version":"prices-v2",
                "rules":[{"rule_id":"mixed","provider":"example",
                "model":"mixed-v1","currency":"USD","components":[
                {"unit":"image","rate_numerator_minor":1,"rate_denominator":1},
                {"unit":"input_token","rate_numerator_minor":1,"rate_denominator":1000000},
                {"unit":"output_token","rate_numerator_minor":1,"rate_denominator":1000000}]}]
            }"#,
        )
        .unwrap();
        let api = Api::new(
            fixture.repository,
            fixture.artifacts,
            catalog(targets, pricing),
            fixture.scheduler,
            || "2026-08-06T00:00:00Z".into(),
        );
        let mut mixed = request("operation-mixed");
        mixed["model"] = json!("mixed-v1");
        mixed["input"]["input_tokens"] = json!(1);
        mixed["input"]["max_output_tokens"] = json!(1);
        let response = api.handle(
            "POST",
            "/v2/executions",
            Some(&fixture.caller),
            &serde_json::to_vec(&mixed).unwrap(),
        );
        assert_eq!(response.status, 400);
    }

    #[test]
    fn image_count_cannot_exceed_artifact_capacity() {
        let fixture = fixture();
        let mut oversized = request("operation-too-many-images");
        oversized["input"]["image_count"] = json!(5);
        assert_eq!(call_create(&fixture, &oversized).status, 400);
    }

    #[test]
    fn artifact_list_is_redacted_and_download_returns_declared_media_type() {
        let fixture = fixture();
        let created = execution(&call_create(&fixture, &request("operation-1")));
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(RgbaImage::new(1, 1))
            .write_to(&mut Cursor::new(&mut bytes), ImageOutputFormat::Png)
            .unwrap();
        let artifact = fixture
            .artifacts
            .store_image(
                &created.execution_id,
                None,
                "image/png",
                &bytes,
                "2026-08-05T20:01:00Z",
            )
            .unwrap();

        let listed = fixture.api.handle(
            "GET",
            &format!("/v1/executions/{}/artifacts", created.execution_id),
            Some(&fixture.caller),
            &[],
        );
        let body: Value = serde_json::from_slice(&listed.body).unwrap();
        assert_eq!(listed.status, 200);
        assert_eq!(body["artifacts"][0]["artifact_id"], artifact.artifact_id);
        let serialized = body.to_string();
        assert!(!serialized.contains("storage_key"));
        assert!(!serialized.contains("local_fs"));

        let downloaded = fixture.api.handle(
            "GET",
            &format!("/v1/artifacts/{}", artifact.artifact_id),
            Some(&fixture.caller),
            &[],
        );
        assert_eq!(downloaded.status, 200);
        assert_eq!(downloaded.content_type, "image/png");
        assert_eq!(downloaded.body, bytes);
        assert_eq!(
            fixture
                .api
                .handle(
                    "GET",
                    &format!("/v1/artifacts/{}", artifact.artifact_id),
                    Some(&fixture.caller),
                    &[],
                )
                .status,
            200
        );
        assert_eq!(
            fixture
                .api
                .handle("GET", "/v1/artifacts/missing", Some(&fixture.caller), &[],)
                .status,
            404
        );
    }
}
