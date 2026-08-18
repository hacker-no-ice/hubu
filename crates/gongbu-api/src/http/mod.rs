//! Version 1 authenticated Execution and Artifact HTTP contract.
//!
//! Transport adapters authenticate a request, construct an [`AuthenticatedAccount`],
//! and pass the method/path/body here. Account identity is deliberately absent from
//! every request schema.
use crate::{
    artifacts::{ArtifactService, Error as ArtifactError},
    execution_scope::{for_target, ExecutionScope, EXECUTION_SCOPE_SCHEMA_VERSION},
    persistence::{
        Artifact, CreateExecutionParams, Error as PersistenceError, Execution, HubuTokenReference,
        Repository,
    },
    provider::{
        contract::{ContractError, NormalizedRequest, PricingSnapshot},
        registry::ValidatedProviderCatalog,
    },
    provider_targets::{Error as TargetError, ProviderConfigVersion, TargetKey},
    temporal::ExecutionScheduler,
    workflow::{
        AuthorizationAdmissionRequest, HubuActivities, OperatorReconciliationRequest,
        ReconciliationAction,
    },
};
use hubu_executor_contract::{
    AuthorizationAmount, AuthorizationExpiryGuidance, AuthorizationScope, AuthorizationTask,
    AuthorizationWorkload, ExecutionScopeSelector, AUTHORIZATION_SCOPE_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Arc};

pub use gongbu_build_info::API_SCHEMA_VERSION as SCHEMA_VERSION;

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
pub struct CreateExecutionRequest {
    pub schema_version: u32,
    pub operation_key: String,
    pub hubu_authorization_id: String,
    pub hubu_claim_id: Option<String>,
    pub hubu_token_reference: String,
    pub authorization: Money,
    pub input: Value,
    pub input_schema_version: i64,
    pub workload_type: String,
    pub provider: String,
    #[serde(default)]
    pub execution_scope: Option<ExecutionScope>,
    pub adapter: String,
    pub model: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationScopePreviewRequest {
    pub schema_version: u32,
    pub operation_key: String,
    pub input: Value,
    pub input_schema_version: i64,
    pub workload_type: String,
    pub provider: String,
    pub adapter: String,
    pub model: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct HubuAuthorizeRequest {
    pub operation_key: String,
    pub account_id: String,
    pub amount_cents: i64,
    pub currency: String,
    pub reason: String,
    pub execution_scope: ExecutionScopeSelector,
    pub workload_profile: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AuthorizationScopePreviewResponse {
    pub authorization_scope: AuthorizationScope,
    pub hubu_authorize_request: HubuAuthorizeRequest,
}

#[derive(Clone, Debug)]
pub struct AuthorizationScopeContext {
    pub agent_id: String,
    pub expiry_by_workload: HashMap<String, AuthorizationExpiryGuidance>,
}

#[derive(Clone)]
pub struct AuthorizationRuntime {
    pub hubu: Arc<dyn HubuActivities + Send + Sync>,
    pub scope: AuthorizationScopeContext,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
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
    message: String,
    diagnostic: Option<String>,
}

impl ApiError {
    fn new(status: u16, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            diagnostic: None,
        }
    }
    fn unauthorized() -> Self {
        Self::new(401, "unauthorized", "authentication is required")
    }
    fn validation() -> Self {
        Self::new(400, "invalid_request", "request validation failed")
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
    fn authorization_scope(diagnostic: String) -> Self {
        let mut error = Self::new(
            422,
            "authorization_scope_mismatch",
            "Hubu authorization does not match the operator-owned execution scope",
        );
        error.diagnostic = Some(diagnostic);
        error
    }
    fn response(&self) -> HttpResponse {
        json_response(
            self.status,
            &ErrorResponse {
                schema_version: SCHEMA_VERSION,
                error: ErrorBody {
                    code: self.code.into(),
                    message: self.message.clone(),
                    diagnostic: self.diagnostic.clone(),
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
    authorization: Option<AuthorizationRuntime>,
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
            authorization: None,
        }
    }

    pub fn new_for_application(
        repository: Repository,
        artifacts: ArtifactService,
        providers: ValidatedProviderCatalog,
        scheduler: Arc<dyn ExecutionScheduler>,
        maximum_spend_minor: i64,
        authorization: AuthorizationRuntime,
        now: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            repository,
            artifacts,
            providers,
            scheduler,
            now: Arc::new(now),
            maximum_spend_minor,
            authorization: Some(authorization),
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
                    ("POST", ["v1", "authorization-scopes", "preview"]) => {
                        self.preview_authorization_scope(account, body)
                    }
                    ("POST", ["v1", "executions"]) => self.create(account, body),
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
        result.unwrap_or_else(|error| error.response())
    }

    fn preview_authorization_scope(
        &self,
        account: &AuthenticatedAccount,
        body: &[u8],
    ) -> Result<HttpResponse, ApiError> {
        let request: AuthorizationScopePreviewRequest =
            serde_json::from_slice(body).map_err(|_| ApiError::validation())?;
        validate_preview(&request)?;
        let context = self
            .authorization
            .as_ref()
            .map(|runtime| &runtime.scope)
            .ok_or_else(ApiError::internal)?;
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
        let normalized_input = canonicalize(&request.input);
        let pricing_request = NormalizedRequest {
            provider: resolved.provider.clone(),
            model: resolved.model.clone(),
            image_count: input_quantity(&normalized_input, "image_count")?,
            input_tokens: input_quantity(&normalized_input, "input_tokens")?,
            max_output_tokens: input_quantity(&normalized_input, "max_output_tokens")?,
            image_size: normalized_input
                .get("image_size")
                .and_then(Value::as_str)
                .map(str::to_owned),
        };
        let pricing = self
            .providers
            .pricing()
            .snapshot_for_target(&resolved.target_key(), &pricing_request)
            .map_err(map_pricing_error)?;
        if pricing.estimated_amount_minor > self.maximum_spend_minor || !pricing.is_image_only() {
            return Err(ApiError::validation());
        }
        let execution_scope =
            for_target(&resolved.provider, &resolved.adapter).ok_or_else(ApiError::validation)?;
        let expiry = context
            .expiry_by_workload
            .get(&resolved.workload_type)
            .cloned()
            .ok_or_else(ApiError::validation)?;
        let preview = build_authorization_scope_preview(
            &account.account_id,
            &context.agent_id,
            request.operation_key.trim(),
            pricing.estimated_amount_minor,
            &pricing.currency,
            execution_scope,
            &resolved.workload_type,
            expiry,
        );
        Ok(json_response(200, &preview))
    }

    fn create(
        &self,
        account: &AuthenticatedAccount,
        body: &[u8],
    ) -> Result<HttpResponse, ApiError> {
        let request: CreateExecutionRequest =
            serde_json::from_slice(body).map_err(|_| ApiError::validation())?;
        validate_create(&request)?;
        let normalized_input = canonicalize(&request.input);
        match self
            .repository
            .get_execution_by_operation(&account.account_id, request.operation_key.trim())
        {
            Ok(existing) if immutable_request_matches(&existing, &request, &normalized_input) => {
                if existing.status == "pending" {
                    self.scheduler
                        .schedule(&existing.execution_id)
                        .map_err(|_| ApiError::internal())?;
                }
                return Ok(json_response(200, &execution_response(existing)?));
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
        if pricing_snapshot.estimated_amount_minor > self.maximum_spend_minor
            || request.authorization.amount_minor > self.maximum_spend_minor
        {
            return Err(ApiError::validation());
        }
        // Tokenization belongs to provider-bound request normalization, which is
        // not part of this HTTP/persistence milestone. Fail closed rather than
        // accepting a caller-asserted token count that could underfund execution.
        if !pricing_snapshot.is_image_only() {
            return Err(ApiError::validation());
        }
        if request.authorization.amount_minor != pricing_snapshot.estimated_amount_minor
            || !request
                .authorization
                .currency
                .eq_ignore_ascii_case(&pricing_snapshot.currency)
        {
            return Err(ApiError::authorization_scope(
                "submitted authorization amount and currency must exactly match the operator pricing catalog"
                    .into(),
            ));
        }
        let canonical_execution_scope = for_target(&resolved.provider, &resolved.adapter)
            .or_else(|| request.execution_scope.clone());
        if request
            .execution_scope
            .as_ref()
            .is_some_and(|supplied| canonical_execution_scope.as_ref() != Some(supplied))
        {
            return Err(ApiError::validation());
        }
        if let Some(authorization) = &self.authorization {
            let execution_scope = canonical_execution_scope
                .clone()
                .ok_or_else(ApiError::validation)?;
            authorization
                .hubu
                .validate_before_admission(&AuthorizationAdmissionRequest {
                    spend_auth_token_id: request.hubu_token_reference.trim().to_owned(),
                    account_id: account.account_id.clone(),
                    amount_minor: request.authorization.amount_minor,
                    execution_scope,
                    operation_key: request.operation_key.trim().to_owned(),
                })
                .map_err(|error| ApiError::authorization_scope(error.diagnostic))?;
        }
        let input_hash = immutable_hash(&request, resolved, &pricing_snapshot, &normalized_input)?;
        let pricing_schema_version = i64::from(pricing_snapshot.schema_version);
        let params = CreateExecutionParams {
            account_id: account.account_id.clone(),
            operation_key: request.operation_key.trim().to_owned(),
            hubu_authorization_id: request.hubu_authorization_id,
            hubu_claim_id: request.hubu_claim_id,
            hubu_token_reference: HubuTokenReference::new(request.hubu_token_reference)
                .map_err(|_| ApiError::validation())?,
            authorized_minor: request.authorization.amount_minor,
            authorization_currency: request.authorization.currency.to_ascii_uppercase(),
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
            execution_scope: canonical_execution_scope,
            created_at: (self.now)(),
        };
        let execution = self
            .repository
            .create_execution(&params)
            .map_err(map_persistence)?;
        if !immutable_params_match(&execution, &params) {
            return Err(ApiError::conflict());
        }
        self.scheduler
            .schedule(&execution.execution_id)
            .map_err(|_| ApiError::internal())?;
        Ok(json_response(200, &execution_response(execution)?))
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
        if request.schema_version != SCHEMA_VERSION
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
        Ok(json_response(202, &execution_response(execution)?))
    }

    fn get_execution(
        &self,
        account: &AuthenticatedAccount,
        execution_id: &str,
    ) -> Result<HttpResponse, ApiError> {
        Ok(json_response(
            200,
            &execution_response(self.authorized_execution(account, execution_id)?)?,
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
                schema_version: SCHEMA_VERSION,
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

#[allow(clippy::too_many_arguments)]
fn build_authorization_scope_preview(
    account_id: &str,
    agent_id: &str,
    operation_key: &str,
    amount_minor: i64,
    currency: &str,
    execution_scope: ExecutionScope,
    workload_type: &str,
    expiry: AuthorizationExpiryGuidance,
) -> AuthorizationScopePreviewResponse {
    let operation_key = operation_key.to_owned();
    let currency = currency.to_ascii_uppercase();
    AuthorizationScopePreviewResponse {
        authorization_scope: AuthorizationScope {
            schema_version: AUTHORIZATION_SCOPE_SCHEMA_VERSION,
            executor_contract: gongbu_build_info::HUBU_EXECUTOR_CONTRACT.into(),
            account_id: account_id.into(),
            agent_id: agent_id.into(),
            operation_key: operation_key.clone(),
            authorization: AuthorizationAmount {
                amount_minor,
                currency: currency.clone(),
            },
            execution_scope: execution_scope.clone(),
            task: AuthorizationTask {
                task_id: operation_key.clone(),
                reason: operation_key.clone(),
                semantics: "reason is the executor task id and must equal operation_key".into(),
            },
            workload: AuthorizationWorkload {
                workload_type: workload_type.into(),
                profile: workload_type.into(),
            },
            expiry,
        },
        hubu_authorize_request: HubuAuthorizeRequest {
            operation_key: operation_key.clone(),
            account_id: account_id.into(),
            amount_cents: amount_minor,
            currency,
            reason: operation_key,
            execution_scope: ExecutionScopeSelector {
                schema_version: execution_scope.schema_version,
                provider: execution_scope.provider.id,
                executor: execution_scope.executor.id,
                capability: execution_scope.capability.id,
                billing_merchant: execution_scope.billing_merchant.id,
            },
            workload_profile: workload_type.into(),
        },
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
    execution.hubu_authorization_id == request.hubu_authorization_id
        // A null request claim delegates claim creation to Gongbu. The durable
        // workflow subsequently records that claim on the execution, so an
        // otherwise identical replay must not conflict with that mutation.
        && request
            .hubu_claim_id
            .as_ref()
            .is_none_or(|claim_id| execution.hubu_claim_id.as_ref() == Some(claim_id))
        && execution.hubu_token_reference.as_str() == request.hubu_token_reference.trim()
        && execution.authorized_minor == request.authorization.amount_minor
        && execution
            .authorization_currency
            .eq_ignore_ascii_case(&request.authorization.currency)
        && &execution.normalized_input == normalized_input
        && execution.input_schema_version == request.input_schema_version
        && execution.workload_type == request.workload_type
        && execution.provider == request.provider
        && request.execution_scope.as_ref().is_none_or(|scope| for_target(&request.provider, &request.adapter).as_ref() == Some(scope))
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

fn validate_create(request: &CreateExecutionRequest) -> Result<(), ApiError> {
    let currency = request.authorization.currency.as_bytes();
    if request.schema_version != SCHEMA_VERSION
        || request.operation_key.trim().is_empty()
        || request.operation_key.len() > 255
        || request.hubu_authorization_id.trim().is_empty()
        || request.authorization.amount_minor < 0
        || currency.len() != 3
        || !currency.iter().all(u8::is_ascii_alphabetic)
        || request.input_schema_version < 1
        || !request.input.is_object()
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
    if let Some(scope) = &request.execution_scope {
        if scope.schema_version != EXECUTION_SCOPE_SCHEMA_VERSION
            || for_target(&request.provider, &request.adapter).as_ref() != Some(scope)
        {
            return Err(ApiError::validation());
        }
    }
    Ok(())
}

fn validate_preview(request: &AuthorizationScopePreviewRequest) -> Result<(), ApiError> {
    if request.schema_version != SCHEMA_VERSION
        || request.operation_key.trim().is_empty()
        || request.operation_key.len() > 255
        || request.input_schema_version < 1
        || !request.input.is_object()
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
    Ok(())
}

fn immutable_hash(
    request: &CreateExecutionRequest,
    resolved: &ProviderConfigVersion,
    pricing_snapshot: &PricingSnapshot,
    normalized_input: &Value,
) -> Result<String, ApiError> {
    let scope = json!({
        "hubu_authorization_id": request.hubu_authorization_id,
        "hubu_claim_id": request.hubu_claim_id,
        "hubu_token_reference": request.hubu_token_reference,
        "authorization": request.authorization,
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

fn execution_response(execution: Execution) -> Result<ExecutionResponse, ApiError> {
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
        schema_version: SCHEMA_VERSION,
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
    use std::{io::Cursor, sync::Barrier, thread};
    use tempfile::TempDir;

    struct AdmissionAdapter(&'static str);
    impl ProviderAdapter for AdmissionAdapter {
        fn adapter_id(&self) -> &str {
            self.0
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
        registry.register("example", "fixture", |_| {
            Ok(Arc::new(AdmissionAdapter("fixture")))
        });
        registry.register("google", "gemini_developer_image", |_| {
            Ok(Arc::new(AdmissionAdapter("gemini_developer_image")))
        });
        ValidatedProviderCatalog::bind(targets, pricing, &registry).unwrap()
    }

    #[test]
    fn authorization_preview_matches_the_versioned_cross_component_fixture() {
        let fixture: AuthorizationScope = serde_json::from_str(include_str!(
            "../../../../fixtures/hubu-authorization-scope-v1.json"
        ))
        .unwrap();
        let preview = build_authorization_scope_preview(
            &fixture.account_id,
            &fixture.agent_id,
            &fixture.operation_key,
            fixture.authorization.amount_minor,
            &fixture.authorization.currency,
            fixture.execution_scope.clone(),
            &fixture.workload.workload_type,
            fixture.expiry.clone(),
        );
        assert_eq!(preview.authorization_scope, fixture);
        assert_eq!(
            preview.hubu_authorize_request.reason,
            preview.hubu_authorize_request.operation_key
        );
        assert_eq!(
            preview.hubu_authorize_request.execution_scope.provider,
            preview.authorization_scope.execution_scope.provider.id
        );
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
        _root: TempDir,
    }

    struct RejectingHubu;

    impl HubuActivities for RejectingHubu {
        fn validate_before_admission(
            &self,
            _: &AuthorizationAdmissionRequest,
        ) -> Result<(), crate::workflow::AuthorizationAdmissionError> {
            Err(crate::workflow::AuthorizationAdmissionError {
                diagnostic: "amount differs from the authorized maximum; issue a new token".into(),
            })
        }

        fn preflight(&self, _: &Execution) -> Result<(), crate::workflow::ActivityError> {
            unreachable!()
        }
        fn claim(&self, _: &Execution) -> Result<String, crate::workflow::ActivityError> {
            unreachable!()
        }
        fn validate_claim(&self, _: &Execution) -> Result<(), crate::workflow::ActivityError> {
            unreachable!()
        }
        fn settle(
            &self,
            _: &Execution,
            _: &str,
            _: i64,
        ) -> Result<String, crate::workflow::ActivityError> {
            unreachable!()
        }
        fn release(&self, _: &Execution) -> Result<(), crate::workflow::ActivityError> {
            unreachable!()
        }
    }

    #[derive(Default)]
    struct RecordingHubu(std::sync::atomic::AtomicUsize);

    impl HubuActivities for RecordingHubu {
        fn validate_before_admission(
            &self,
            _: &AuthorizationAdmissionRequest,
        ) -> Result<(), crate::workflow::AuthorizationAdmissionError> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }

        fn preflight(&self, _: &Execution) -> Result<(), crate::workflow::ActivityError> {
            unreachable!()
        }
        fn claim(&self, _: &Execution) -> Result<String, crate::workflow::ActivityError> {
            unreachable!()
        }
        fn validate_claim(&self, _: &Execution) -> Result<(), crate::workflow::ActivityError> {
            unreachable!()
        }
        fn settle(
            &self,
            _: &Execution,
            _: &str,
            _: i64,
        ) -> Result<String, crate::workflow::ActivityError> {
            unreachable!()
        }
        fn release(&self, _: &Execution) -> Result<(), crate::workflow::ActivityError> {
            unreachable!()
        }
    }

    fn fixture() -> Fixture {
        let repository = Repository::in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let artifacts = ArtifactService::new(
            repository.clone(),
            LocalFsStorage::new(root.path()),
            ArtifactLimits::default(),
        );
        let targets: ProviderTargetConfig = serde_json::from_value(json!({
            "schema_version": 2,
            "provider_configs": [{
                "provider_config_version": "provider-v1",
                "workload_type": "image_generation",
                "provider": "example",
                "adapter": "fixture",
                "model": "image-v1",
                "secret_service": "gongbu.example",
                "secret_account": "local",
                "settings": {"type": "fixture"}
            }, {
                "provider_config_version": "gemini-v1",
                "workload_type": "image_generation",
                "provider": "google",
                "adapter": "gemini_developer_image",
                "model": "gemini-3.1-flash-image-preview",
                "secret_service": "gongbu.google",
                "secret_account": "local",
                "settings": {
                    "type": "gemini_developer_image",
                    "config": {
                        "endpoint": "https://generativelanguage.googleapis.com",
                        "api_version": "v1beta",
                        "timeout_ms": 1000,
                        "headers": {}
                    }
                }
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
                }, {
                    "rule_id":"gemini-image",
                    "provider":"google",
                    "model":"gemini-3.1-flash-image-preview",
                    "currency":"USD",
                    "unit":"image",
                    "unit_amount_minor":10
                }]
            }"#,
        )
        .unwrap();
        let scheduler = Arc::new(Scheduler::default());
        Fixture {
            api: Api::new(
                repository.clone(),
                artifacts.clone(),
                catalog(targets, pricing),
                scheduler.clone(),
                || "2026-08-05T20:00:00Z".into(),
            ),
            repository,
            artifacts,
            owner: AuthenticatedAccount::from_verified_claim("account-a").unwrap(),
            other: AuthenticatedAccount::from_verified_claim("account-b").unwrap(),
            scheduler,
            _root: root,
        }
    }

    #[test]
    fn operator_maximum_spend_rejects_authorization_and_price_before_persistence() {
        let fixture = fixture();
        let api = Api::new_with_maximum_spend(
            fixture.repository.clone(),
            fixture.artifacts.clone(),
            fixture.api.providers.clone(),
            fixture.scheduler.clone(),
            100,
            || "2026-08-05T20:00:00Z".into(),
        );
        let mut over_ceiling = request("over-ceiling");
        over_ceiling["authorization"]["amount_minor"] = json!(101);
        let response = api.handle(
            "POST",
            "/v1/executions",
            Some(&fixture.owner),
            &serde_json::to_vec(&over_ceiling).unwrap(),
        );
        assert_eq!(response.status, 400);
        assert!(fixture
            .repository
            .get_execution_by_operation("account-a", "over-ceiling")
            .is_err());

        let api = Api::new_with_maximum_spend(
            fixture.repository.clone(),
            fixture.artifacts.clone(),
            fixture.api.providers.clone(),
            fixture.scheduler.clone(),
            99,
            || "2026-08-05T20:00:00Z".into(),
        );
        let mut request = request("price-over-ceiling");
        request["authorization"]["amount_minor"] = json!(99);
        let response = api.handle(
            "POST",
            "/v1/executions",
            Some(&fixture.owner),
            &serde_json::to_vec(&request).unwrap(),
        );
        assert_eq!(response.status, 400);
    }

    fn request(operation_key: &str) -> Value {
        json!({
            "schema_version": 1,
            "operation_key": operation_key,
            "hubu_authorization_id": "auth-1",
            "hubu_claim_id": "claim-1",
            "hubu_token_reference": "sha256:opaque-reference",
            "authorization": {"amount_minor": 100, "currency": "USD"},
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

    fn call_create(fixture: &Fixture, request: &Value) -> HttpResponse {
        fixture.api.handle(
            "POST",
            "/v1/executions",
            Some(&fixture.owner),
            &serde_json::to_vec(request).unwrap(),
        )
    }

    fn scoped_request(operation_key: &str) -> Value {
        let mut request = request(operation_key);
        request["authorization"]["amount_minor"] = json!(10);
        request["provider"] = json!("google");
        request["adapter"] = json!("gemini_developer_image");
        request["model"] = json!("gemini-3.1-flash-image-preview");
        request
    }

    fn authorization_context() -> AuthorizationScopeContext {
        AuthorizationScopeContext {
            agent_id: "agt_gongbu_executor".into(),
            expiry_by_workload: HashMap::from([(
                "image_generation".into(),
                AuthorizationExpiryGuidance {
                    authorization_ttl_seconds: 300,
                    claim_ttl_seconds: 900,
                    guidance: "issue immediately before admission; Gongbu must claim before authorization expiry".into(),
                },
            )]),
        }
    }

    #[test]
    fn authenticated_preview_derives_operator_owned_scope() {
        let fixture = fixture();
        let owner = fixture.owner.clone();
        let api = Api::new_for_application(
            fixture.repository,
            fixture.artifacts,
            fixture.api.providers,
            fixture.scheduler,
            100,
            AuthorizationRuntime {
                hubu: Arc::new(RejectingHubu),
                scope: authorization_context(),
            },
            || "2026-08-05T20:00:00Z".into(),
        );
        let planned = scoped_request("preview-op");
        let request = json!({
            "schema_version": 1,
            "operation_key": planned["operation_key"],
            "input": planned["input"],
            "input_schema_version": planned["input_schema_version"],
            "workload_type": planned["workload_type"],
            "provider": planned["provider"],
            "adapter": planned["adapter"],
            "model": planned["model"]
        });
        let response = api.handle(
            "POST",
            "/v1/authorization-scopes/preview",
            Some(&owner),
            &serde_json::to_vec(&request).unwrap(),
        );
        assert_eq!(response.status, 200);
        let preview: AuthorizationScopePreviewResponse =
            serde_json::from_slice(&response.body).unwrap();
        assert_eq!(preview.authorization_scope.account_id, "account-a");
        assert_eq!(preview.authorization_scope.agent_id, "agt_gongbu_executor");
        assert_eq!(preview.authorization_scope.authorization.amount_minor, 10);
        assert_eq!(
            preview.authorization_scope.execution_scope,
            for_target("google", "gemini_developer_image").unwrap()
        );
    }

    #[test]
    fn hubu_mismatch_fails_before_persistence_or_scheduling_with_diagnostics() {
        let fixture = fixture();
        let owner = fixture.owner.clone();
        let repository = fixture.repository.clone();
        let scheduler = fixture.scheduler.clone();
        let api = Api::new_for_application(
            fixture.repository,
            fixture.artifacts,
            fixture.api.providers,
            scheduler.clone(),
            100,
            AuthorizationRuntime {
                hubu: Arc::new(RejectingHubu),
                scope: authorization_context(),
            },
            || "2026-08-05T20:00:00Z".into(),
        );
        let response = api.handle(
            "POST",
            "/v1/executions",
            Some(&owner),
            &serde_json::to_vec(&scoped_request("scope-mismatch")).unwrap(),
        );
        assert_eq!(response.status, 422);
        let error: ErrorResponse = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(error.error.code, "authorization_scope_mismatch");
        assert!(error
            .error
            .diagnostic
            .as_deref()
            .unwrap()
            .contains("issue a new token"));
        assert!(repository
            .get_execution_by_operation("account-a", "scope-mismatch")
            .is_err());
        assert!(scheduler.0.lock().unwrap().is_empty());
    }

    #[test]
    fn admission_requires_catalog_money_before_hubu_persistence_or_scheduling() {
        let fixture = fixture();
        let owner = fixture.owner.clone();
        let repository = fixture.repository.clone();
        let scheduler = fixture.scheduler.clone();
        let hubu = Arc::new(RecordingHubu::default());
        let api = Api::new_for_application(
            fixture.repository,
            fixture.artifacts,
            fixture.api.providers,
            scheduler.clone(),
            100,
            AuthorizationRuntime {
                hubu: hubu.clone(),
                scope: authorization_context(),
            },
            || "2026-08-05T20:00:00Z".into(),
        );

        for (operation_key, amount_minor, currency) in [
            ("inflated-amount", 11, "USD"),
            ("wrong-currency", 10, "EUR"),
        ] {
            let mut request = scoped_request(operation_key);
            request["authorization"]["amount_minor"] = json!(amount_minor);
            request["authorization"]["currency"] = json!(currency);
            let response = api.handle(
                "POST",
                "/v1/executions",
                Some(&owner),
                &serde_json::to_vec(&request).unwrap(),
            );
            assert_eq!(response.status, 422);
            let error: ErrorResponse = serde_json::from_slice(&response.body).unwrap();
            assert_eq!(error.error.code, "authorization_scope_mismatch");
            assert!(error
                .error
                .diagnostic
                .as_deref()
                .unwrap()
                .contains("operator pricing catalog"));
            assert!(repository
                .get_execution_by_operation("account-a", operation_key)
                .is_err());
        }

        assert_eq!(hubu.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(scheduler.0.lock().unwrap().is_empty());
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
        assert_eq!(first.schema_version, 1);
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
        assert_eq!(execution(&fetched), first);
    }

    #[test]
    fn replay_with_gongbu_managed_claim_matches_after_claim_is_recorded() {
        let fixture = fixture();
        let mut submitted = request("managed-claim-replay");
        submitted["hubu_claim_id"] = Value::Null;
        let created = execution(&call_create(&fixture, &submitted));
        let mut persisted = fixture
            .repository
            .get_execution(&created.execution_id)
            .unwrap();
        persisted.hubu_claim_id = Some("claim-created-by-workflow".into());
        let decoded: CreateExecutionRequest = serde_json::from_value(submitted).unwrap();

        assert!(immutable_request_matches(
            &persisted,
            &decoded,
            &canonicalize(&decoded.input)
        ));
    }

    #[test]
    fn authenticated_operator_reconciliation_signals_stable_execution() {
        let fixture = fixture();
        let created: ExecutionResponse = serde_json::from_slice(
            &fixture
                .api
                .handle(
                    "POST",
                    "/v1/executions",
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
                .handle("POST", "/v1/executions", None, b"{}")
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
            "/v1/executions",
            Some(&fixture.owner),
            &serde_json::to_vec(&request("operation-1")).unwrap(),
        );
        assert_eq!(replay.status, 200);
        assert_eq!(execution(&replay).execution_id, created.execution_id);
    }

    #[test]
    fn concurrent_create_replays_across_operator_snapshot_changes() {
        let fixture = fixture();
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
                "unit_amount_minor":100}]
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
                    api.handle("POST", "/v1/executions", Some(&owner), &body)
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
            "/v1/executions",
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
            "/v1/executions",
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
