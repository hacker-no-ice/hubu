//! Versioned authenticated Execution and Artifact HTTP contract.
//!
//! Transport adapters authenticate a request, construct an [`AuthenticatedAccount`],
//! and pass the method/path/body here. Account identity is deliberately absent from
//! every request schema.
use crate::{
    artifacts::{ArtifactService, Error as ArtifactError},
    execution_scope::{for_target, ExecutionScope},
    hubu::{HttpClientError, SpendAuthorizationResolver},
    persistence::{
        Artifact, CreateExecutionParams, Error as PersistenceError, Execution,
        HubuAuthorizationSnapshot, HubuTokenReference, Repository,
    },
    provider::{
        contract::{ContractError, NormalizedRequest, PricingSnapshot},
        registry::ValidatedProviderCatalog,
    },
    provider_targets::{Error as TargetError, ProviderConfigVersion, TargetKey},
    temporal::ExecutionScheduler,
    workflow::{OperatorReconciliationRequest, ReconciliationAction},
};
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub use gongbu_build_info::API_SCHEMA_VERSION as SCHEMA_VERSION;
pub const V1_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthenticatedAccount {
    account_id: String,
}

impl AuthenticatedAccount {
    /// Construct only from a successfully validated authentication claim.
    pub fn from_verified_claim(account_id: impl Into<String>) -> Result<Self, ApiError> {
        let account_id = account_id.into();
        let account_id = account_id.trim();
        if account_id.is_empty() || account_id.len() > 255 {
            return Err(ApiError::unauthorized());
        }
        Ok(Self {
            account_id: account_id.to_owned(),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateExecutionV1Request {
    pub schema_version: u32,
    pub operation_key: String,
    /// Historical alias for the Hubu spend authorization token identifier.
    pub hubu_authorization_id: String,
    pub hubu_claim_id: Option<String>,
    /// Historical alias for the same token identifier as `hubu_authorization_id`.
    pub hubu_token_reference: String,
    pub authorization: Money,
    #[serde(default)]
    pub execution_scope: Option<ExecutionScope>,
    pub input: Value,
    pub input_schema_version: i64,
    pub workload_type: String,
    pub provider: String,
    pub adapter: String,
    pub model: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateExecutionV2Request {
    pub schema_version: u32,
    pub spend_auth_token_id: String,
    pub input: Value,
    pub input_schema_version: i64,
    pub workload_type: String,
    pub provider: String,
    pub adapter: String,
    pub model: String,
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ErrorResponse {
    pub schema_version: u32,
    pub error: ErrorBody,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
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
}

impl ApiError {
    fn new(status: u16, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
        }
    }
    fn unauthorized() -> Self {
        Self::new(401, "unauthorized", "authentication is required")
    }
    fn validation() -> Self {
        Self::new(400, "invalid_request", "request validation failed")
    }
    fn legacy_token_alias_mismatch() -> Self {
        Self::new(
            400,
            "legacy_token_alias_mismatch",
            "v1 Hubu token identifier aliases must be equal",
        )
    }
    fn not_found() -> Self {
        Self::new(404, "not_found", "resource not found")
    }
    fn forbidden() -> Self {
        Self::new(403, "forbidden", "resource belongs to another account")
    }
    fn conflict() -> Self {
        Self::new(
            409,
            "immutable_scope_conflict",
            "operation key was already used with different immutable input",
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
        }
    }

    pub fn handle(
        &self,
        method: &str,
        path: &str,
        account: Option<&AuthenticatedAccount>,
        body: &[u8],
    ) -> HttpResponse {
        let result = account
            .ok_or_else(ApiError::unauthorized)
            .and_then(|account| {
                let segments: Vec<_> = path.trim_matches('/').split('/').collect();
                match (method, segments.as_slice()) {
                    ("POST", ["v1", "executions"]) => self.create_v1(account, body),
                    ("POST", ["v2", "executions"]) => self.create_v2(account, body),
                    ("GET", ["v1", "executions", execution_id]) => {
                        self.get_execution(account, execution_id)
                    }
                    ("POST", ["v1", "executions", execution_id, "reconciliation"]) => {
                        self.reconcile(account, execution_id, body)
                    }
                    ("GET", ["v1", "executions", execution_id, "artifacts"]) => {
                        self.list_artifacts(account, execution_id)
                    }
                    ("GET", ["v1", "artifacts", artifact_id]) => {
                        self.get_artifact(account, artifact_id)
                    }
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

    fn create_v1(
        &self,
        account: &AuthenticatedAccount,
        body: &[u8],
    ) -> Result<HttpResponse, ApiError> {
        let request: CreateExecutionV1Request =
            serde_json::from_slice(body).map_err(|_| ApiError::validation())?;
        let request = translate_v1(request)?;
        self.create(account, request, V1_SCHEMA_VERSION)
    }

    fn create_v2(
        &self,
        account: &AuthenticatedAccount,
        body: &[u8],
    ) -> Result<HttpResponse, ApiError> {
        let request: CreateExecutionV2Request =
            serde_json::from_slice(body).map_err(|_| ApiError::validation())?;
        let request = translate_v2(request)?;
        self.create(account, request, SCHEMA_VERSION)
    }

    fn create(
        &self,
        account: &AuthenticatedAccount,
        request: CreateExecutionRequest,
        response_schema_version: u32,
    ) -> Result<HttpResponse, ApiError> {
        validate_create(&request)?;
        let spend_auth_token_id = request.spend_auth_token_id.clone();
        let normalized_input = canonicalize(&request.input);
        match self
            .repository
            .get_execution_by_hubu_token(&account.account_id, &spend_auth_token_id)
        {
            Ok(existing) if immutable_request_matches(&existing, &request, &normalized_input) => {
                if existing.status == "pending" {
                    self.scheduler
                        .schedule(&existing.execution_id)
                        .map_err(|_| ApiError::internal())?;
                }
                return Ok(json_response(
                    200,
                    &execution_response(existing, response_schema_version)?,
                ));
            }
            Ok(_) => return Err(ApiError::conflict()),
            Err(PersistenceError::NotFound) => {}
            Err(error) => return Err(map_persistence(error)),
        }
        let target_key = TargetKey::new(
            &request.workload_type,
            &request.provider,
            &request.adapter,
            &request.model,
        )
        .map_err(map_target_error)?;
        let resolved = self
            .providers
            .resolve_active(&target_key)
            .map_err(|_| ApiError::validation())?;
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
        if authorization.spend_auth_token_id != spend_auth_token_id
            || authorization.status != "available"
            || authorization_expires_at <= admission_time
            || authorization.budget_hold.status != "frozen"
            || authorization.budget_hold.amount_cents != authorization.amount_cents
            || authorization.budget_hold.frozen_amount_cents != authorization.amount_cents
            || authorization.budget_hold.consumed_amount_cents != 0
            || authorization.account_id != account.account_id
            || authorization.operation_key.trim().is_empty()
            || authorization.decision_id.trim().is_empty()
            || authorization.amount_cents != pricing_snapshot.estimated_amount_minor
            || !authorization
                .currency
                .eq_ignore_ascii_case(&pricing_snapshot.currency)
            || authorization.execution_scope.as_ref() != Some(&execution_scope)
            || authorization.workload_profile != request.workload_type
            || !legacy_authorization_matches(
                &request,
                &authorization,
                &execution_scope,
                &pricing_snapshot,
            )
        {
            return Err(ApiError::validation());
        }
        match self
            .repository
            .get_execution_by_operation(&account.account_id, authorization.operation_key.trim())
        {
            Ok(_) => return Err(ApiError::conflict()),
            Err(PersistenceError::NotFound) => {}
            Err(error) => return Err(map_persistence(error)),
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
            workload_profile: authorization.workload_profile.clone(),
            expires_at: authorization.expires_at.clone(),
            authorization_status: authorization.status.clone(),
            task_id: authorization.task_id.clone(),
            reason: authorization.reason.clone(),
        };
        let pricing_schema_version = i64::from(pricing_snapshot.schema_version);
        let params = CreateExecutionParams {
            account_id: account.account_id.clone(),
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
            .map_err(|_| ApiError::internal())?;
        Ok(json_response(
            200,
            &execution_response(execution, response_schema_version)?,
        ))
    }

    fn authorized_execution(
        &self,
        account: &AuthenticatedAccount,
        execution_id: &str,
    ) -> Result<Execution, ApiError> {
        let execution = self
            .repository
            .get_execution(execution_id)
            .map_err(map_persistence)?;
        if execution.account_id != account.account_id {
            return Err(ApiError::forbidden());
        }
        Ok(execution)
    }

    fn reconcile(
        &self,
        account: &AuthenticatedAccount,
        execution_id: &str,
        body: &[u8],
    ) -> Result<HttpResponse, ApiError> {
        let execution = self.authorized_execution(account, execution_id)?;
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
            &execution_response(execution, V1_SCHEMA_VERSION)?,
        ))
    }

    fn get_execution(
        &self,
        account: &AuthenticatedAccount,
        execution_id: &str,
    ) -> Result<HttpResponse, ApiError> {
        Ok(json_response(
            200,
            &execution_response(
                self.authorized_execution(account, execution_id)?,
                V1_SCHEMA_VERSION,
            )?,
        ))
    }

    fn list_artifacts(
        &self,
        account: &AuthenticatedAccount,
        execution_id: &str,
    ) -> Result<HttpResponse, ApiError> {
        self.authorized_execution(account, execution_id)?;
        let artifacts = self
            .artifacts
            .list_for_account(execution_id, &account.account_id)
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

    fn get_artifact(
        &self,
        account: &AuthenticatedAccount,
        artifact_id: &str,
    ) -> Result<HttpResponse, ApiError> {
        let artifact = self
            .repository
            .get_artifact(artifact_id)
            .map_err(map_persistence)?;
        self.authorized_execution(account, &artifact.execution_id)?;
        let retrieved = self
            .artifacts
            .retrieve_for_account(artifact_id, &account.account_id)
            .map_err(map_artifact_error)?;
        Ok(HttpResponse {
            status: 200,
            content_type: retrieved.artifact.media_type,
            body: retrieved.bytes,
        })
    }
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

fn translate_v1(request: CreateExecutionV1Request) -> Result<CreateExecutionRequest, ApiError> {
    let authorization_id = request.hubu_authorization_id.trim();
    let token_reference = request.hubu_token_reference.trim();
    if request.schema_version != V1_SCHEMA_VERSION || authorization_id.is_empty() {
        return Err(ApiError::validation());
    }
    if authorization_id != token_reference {
        return Err(ApiError::legacy_token_alias_mismatch());
    }
    Ok(CreateExecutionRequest {
        spend_auth_token_id: authorization_id.to_owned(),
        operation_key: Some(request.operation_key),
        hubu_claim_id: request.hubu_claim_id,
        authorization: Some(request.authorization),
        execution_scope: request.execution_scope,
        input: request.input,
        input_schema_version: request.input_schema_version,
        workload_type: request.workload_type,
        provider: request.provider,
        adapter: request.adapter,
        model: request.model,
    })
}

fn translate_v2(request: CreateExecutionV2Request) -> Result<CreateExecutionRequest, ApiError> {
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
        workload_type: request.workload_type,
        provider: request.provider,
        adapter: request.adapter,
        model: request.model,
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
        "workload_profile": authorization.workload_profile,
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
    })
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
        ContractError::UnsupportedTarget
        | ContractError::IndeterminableCost
        | ContractError::InsufficientAuthorization => ApiError::validation(),
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
        provider::{
            contract::{
                AdapterCapabilities, AdapterOutcome, PricingCatalog, ProviderAdapter,
                ProviderFailure,
            },
            registry::ProviderRegistry,
        },
        provider_targets::ProviderTargetConfig,
        secrets::ProviderSecret,
    };
    use image::{DynamicImage, ImageOutputFormat, RgbaImage};
    use serde_json::json;
    use std::{
        io::Cursor,
        sync::{
            atomic::{AtomicUsize, Ordering},
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

    fn catalog(targets: ProviderTargetConfig, pricing: PricingCatalog) -> ValidatedProviderCatalog {
        let mut registry = ProviderRegistry::new();
        registry.register("example", "fixture", |_| Ok(Arc::new(AdmissionAdapter)));
        ValidatedProviderCatalog::bind(targets, pricing, &registry).unwrap()
    }

    #[derive(Default)]
    struct Scheduler(std::sync::Mutex<Vec<String>>);
    impl ExecutionScheduler for Scheduler {
        fn schedule(&self, execution_id: &str) -> Result<(), String> {
            self.0.lock().unwrap().push(execution_id.into());
            Ok(())
        }
        fn reconcile(
            &self,
            execution_id: &str,
            _: OperatorReconciliationRequest,
        ) -> Result<(), String> {
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
        owner: AuthenticatedAccount,
        other: AuthenticatedAccount,
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
            self.calls.fetch_add(1, Ordering::SeqCst);
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
                workload_profile: "image_generation".into(),
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
                response.execution_scope = for_target("google", "gemini_image");
            } else if spend_auth_token_id.starts_with("token-swap") {
                response.spend_auth_token_id = "different-token".into();
            } else if spend_auth_token_id.starts_with("expired") {
                response.expires_at = "2026-08-05T19:00:00Z".into();
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
                "schema_version":1,
                "catalog_version":"prices-v1",
                "rules":[{
                    "rule_id":"example-image",
                    "provider":"example",
                    "model":"image-v1",
                    "currency":"USD",
                    "unit":"image",
                    "unit_amount_minor":100
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
            owner: AuthenticatedAccount::from_verified_claim("account-a").unwrap(),
            other: AuthenticatedAccount::from_verified_claim("account-b").unwrap(),
            scheduler,
            resolver,
            _root: root,
        }
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
            Some(&fixture.owner),
            &serde_json::to_vec(&request("over-ceiling")).unwrap(),
        );
        assert_eq!(response.status, 400);
        assert!(fixture
            .repository
            .get_execution_by_operation("account-a", "over-ceiling")
            .is_err());
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
            "workload_type": "image_generation",
            "provider": "example",
            "adapter": "fixture",
            "model": "image-v1"
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
    fn historical_create_fixture_matches_v1_schema() {
        let raw = include_str!("../../../../fixtures/gongbu-create-execution-v1.json");
        let request: CreateExecutionV1Request = serde_json::from_str(raw).unwrap();
        assert_eq!(request.schema_version, 1);
        assert_eq!(request.hubu_authorization_id, request.hubu_token_reference);
        let fixture = fixture();
        let response = fixture.api.handle(
            "POST",
            "/v1/executions",
            Some(&fixture.owner),
            raw.as_bytes(),
        );
        assert_eq!(response.status, 200);
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.scheduler.0.lock().unwrap().len(), 1);
    }

    fn call_create(fixture: &Fixture, request: &Value) -> HttpResponse {
        fixture.api.handle(
            "POST",
            "/v2/executions",
            Some(&fixture.owner),
            &serde_json::to_vec(request).unwrap(),
        )
    }

    fn call_create_v1(fixture: &Fixture, request: &Value) -> HttpResponse {
        fixture.api.handle(
            "POST",
            "/v1/executions",
            Some(&fixture.owner),
            &serde_json::to_vec(request).unwrap(),
        )
    }

    fn execution(response: &HttpResponse) -> ExecutionResponse {
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
            Some(&fixture.owner),
            &[],
        );
        let fetched = execution(&fetched);
        assert_eq!(fetched.schema_version, 1);
        assert_eq!(fetched.execution_id, first.execution_id);
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
        let decoded = translate_v2(serde_json::from_value(submitted).unwrap()).unwrap();

        assert!(immutable_request_matches(
            &persisted,
            &decoded,
            &canonicalize(&decoded.input)
        ));
    }

    #[test]
    fn v1_historical_envelope_translates_and_alias_mismatch_has_no_side_effects() {
        let fixture = fixture();
        let legacy = legacy_request("legacy-token");
        assert_eq!(call_create_v1(&fixture, &legacy).status, 200);
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), 1);
        let schedules_before = fixture.scheduler.0.lock().unwrap().len();

        let mut unequal = legacy_request("unequal-token");
        unequal["hubu_authorization_id"] = json!("different-token");
        let rejected = call_create_v1(&fixture, &unequal);
        assert_eq!(rejected.status, 400);
        let error: ErrorResponse = serde_json::from_slice(&rejected.body).unwrap();
        assert_eq!(error.schema_version, 1);
        assert_eq!(error.error.code, "legacy_token_alias_mismatch");
        assert!(!error.error.message.contains("unequal-token"));
        assert!(!error.error.message.contains("different-token"));
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.scheduler.0.lock().unwrap().len(), schedules_before);
        assert!(fixture
            .repository
            .get_execution_by_hubu_token("account-a", "unequal-token")
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
            assert_eq!(call_create_v1(&fixture, &request).status, 400, "{name}");
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
            "identity-mismatch-1",
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
        assert_eq!(snapshot.workload_profile, "image_generation");
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
    fn historical_v1_envelope_replays_same_execution_after_restart() {
        let fixture = fixture();
        let submitted = legacy_request("legacy-restart-token");
        let created = execution(&call_create_v1(&fixture, &submitted));
        let calls_before = fixture.resolver.calls.load(Ordering::SeqCst);
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
            Some(&fixture.owner),
            &serde_json::to_vec(&submitted).unwrap(),
        );
        assert_eq!(replay.status, 200);
        assert_eq!(execution(&replay).execution_id, created.execution_id);
        assert_eq!(fixture.resolver.calls.load(Ordering::SeqCst), calls_before);
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
                    Some(&fixture.owner),
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
                    Some(&fixture.other),
                    &body
                )
                .status,
            403
        );
        assert_eq!(
            fixture
                .api
                .handle(
                    "POST",
                    &format!("/v1/executions/{}/reconciliation", created.execution_id),
                    Some(&fixture.owner),
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
    fn authentication_validation_forbidden_and_not_found_are_distinct() {
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
                    Some(&fixture.other),
                    &[],
                )
                .status,
            403
        );
        assert_eq!(
            fixture
                .api
                .handle("GET", "/v1/executions/missing", Some(&fixture.owner), &[],)
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
        fabricated_snapshot["pricing_snapshot"] = json!({"unit_amount_minor": 0});
        assert_eq!(call_create(&fixture, &fabricated_snapshot).status, 400);
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
                "schema_version":1,
                "catalog_version":"prices-v2",
                "rules":[{
                    "rule_id":"example-image-v2",
                    "provider":"example",
                    "model":"image-v1",
                    "currency":"USD",
                    "unit":"image",
                    "unit_amount_minor":200
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
            Some(&fixture.owner),
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
                "schema_version":1,"catalog_version":"prices-v2",
                "rules":[{"rule_id":"example-image-v2","provider":"example",
                "model":"image-v1","currency":"USD","unit":"image",
                "unit_amount_minor":200}]
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
                let owner = fixture.owner.clone();
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
        assert!(responses.iter().all(|response| response.status == 200));
        assert_eq!(
            execution(&responses[0]).execution_id,
            execution(&responses[1]).execution_id
        );
        assert_eq!(execution(&responses[0]).execution_id, created.execution_id);
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
                "schema_version":1,"catalog_version":"prices-v1",
                "rules":[{"rule_id":"example-text","provider":"example",
                "model":"text-v1","currency":"USD","unit":"input_token",
                "unit_amount_minor":1}]
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
            Some(&fixture.owner),
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
            Some(&fixture.owner),
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
            Some(&fixture.owner),
            &[],
        );
        let body: Value = serde_json::from_slice(&listed.body).unwrap();
        assert_eq!(listed.status, 200);
        assert_eq!(body["artifacts"][0]["artifact_id"], artifact.artifact_id);
        assert!(body.to_string().find("storage_key").is_none());
        assert!(body.to_string().find("local_fs").is_none());

        let downloaded = fixture.api.handle(
            "GET",
            &format!("/v1/artifacts/{}", artifact.artifact_id),
            Some(&fixture.owner),
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
                    Some(&fixture.other),
                    &[],
                )
                .status,
            403
        );
        assert_eq!(
            fixture
                .api
                .handle("GET", "/v1/artifacts/missing", Some(&fixture.owner), &[],)
                .status,
            404
        );
    }
}
