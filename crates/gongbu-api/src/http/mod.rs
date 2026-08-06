//! Version 1 authenticated Execution and Artifact HTTP contract.
//!
//! Transport adapters authenticate a request, construct an [`AuthenticatedAccount`],
//! and pass the method/path/body here. Account identity is deliberately absent from
//! every request schema.
use crate::{
    artifacts::{ArtifactService, Error as ArtifactError},
    persistence::{
        Artifact, CreateExecutionParams, Error as PersistenceError, Execution, HubuTokenReference,
        Repository,
    },
    provider_contract::{
        ContractError, NormalizedRequest, PricingCatalog, PricingSnapshot, PricingUnit,
        PRICING_SNAPSHOT_SCHEMA_VERSION,
    },
    provider_targets::{Error as TargetError, ProviderConfigVersion, ProviderTargetConfig},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub const SCHEMA_VERSION: u32 = 1;

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
    pub adapter: String,
    pub model: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Money {
    pub amount_minor: i64,
    pub currency: String,
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
    Persisting,
    Settling,
    Succeeded,
    Released,
    Failed,
    /// Read compatibility for executions cancelled before the v1 contract was frozen.
    Cancelled,
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
    fn response(&self) -> HttpResponse {
        json_response(
            self.status,
            &ErrorResponse {
                schema_version: SCHEMA_VERSION,
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
    targets: ProviderTargetConfig,
    pricing: PricingCatalog,
    now: Arc<dyn Fn() -> String + Send + Sync>,
}

impl Api {
    pub fn new(
        repository: Repository,
        artifacts: ArtifactService,
        targets: ProviderTargetConfig,
        pricing: PricingCatalog,
        now: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            repository,
            artifacts,
            targets,
            pricing,
            now: Arc::new(now),
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
                    ("POST", ["v1", "executions"]) => self.create(account, body),
                    ("GET", ["v1", "executions", execution_id]) => {
                        self.get_execution(account, execution_id)
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
                return Ok(json_response(200, &execution_response(existing)?));
            }
            Ok(_) => return Err(ApiError::conflict()),
            Err(PersistenceError::NotFound) => {}
            Err(error) => return Err(map_persistence(error)),
        }
        let resolved = self
            .targets
            .resolve(
                &request.workload_type,
                &request.provider,
                &request.adapter,
                &request.model,
            )
            .map_err(map_target_error)?;
        let image_count = input_quantity(&normalized_input, "image_count")?;
        if image_count.is_some_and(|count| {
            u64::try_from(count).map_or(true, |count| {
                count > self.artifacts.max_artifacts_per_execution()
            })
        }) {
            return Err(ApiError::validation());
        }
        let pricing_snapshot = self
            .pricing
            .snapshot(&NormalizedRequest {
                provider: resolved.provider.clone(),
                model: resolved.model.clone(),
                image_count,
                input_tokens: input_quantity(&normalized_input, "input_tokens")?,
                max_output_tokens: input_quantity(&normalized_input, "max_output_tokens")?,
            })
            .map_err(map_pricing_error)?;
        // Tokenization belongs to provider-bound request normalization, which is
        // not part of this HTTP/persistence milestone. Fail closed rather than
        // accepting a caller-asserted token count that could underfund execution.
        if pricing_snapshot.unit != PricingUnit::Image {
            return Err(ApiError::validation());
        }
        let input_hash = immutable_hash(&request, resolved, &pricing_snapshot, &normalized_input)?;
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
            pricing_snapshot: serde_json::to_value(pricing_snapshot)
                .map_err(|_| ApiError::internal())?,
            pricing_schema_version: PRICING_SNAPSHOT_SCHEMA_VERSION,
            created_at: (self.now)(),
        };
        let execution = self
            .repository
            .create_execution(&params)
            .map_err(map_persistence)?;
        if !immutable_params_match(&execution, &params) {
            return Err(ApiError::conflict());
        }
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
        && execution.hubu_claim_id == request.hubu_claim_id
        && execution.hubu_token_reference.as_str() == request.hubu_token_reference.trim()
        && execution.authorized_minor == request.authorization.amount_minor
        && execution
            .authorization_currency
            .eq_ignore_ascii_case(&request.authorization.currency)
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
        "adapter": resolved.adapter,
        "model": resolved.model,
        "provider_config_version": resolved.provider_config_version,
        "pricing_snapshot": pricing_snapshot,
        "pricing_schema_version": PRICING_SNAPSHOT_SCHEMA_VERSION,
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
        "persisting" => ExecutionStatus::Persisting,
        "settling" => ExecutionStatus::Settling,
        "succeeded" => ExecutionStatus::Succeeded,
        "failed" => ExecutionStatus::Failed,
        "released" => ExecutionStatus::Released,
        "cancelled" => ExecutionStatus::Cancelled,
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
        TargetError::Disabled | TargetError::NotConfigured | TargetError::Ambiguous => {
            ApiError::validation()
        }
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
    use crate::artifacts::{ArtifactLimits, LocalFsStorage};
    use image::{DynamicImage, ImageOutputFormat, RgbaImage};
    use serde_json::json;
    use std::{io::Cursor, sync::Barrier, thread};
    use tempfile::TempDir;

    struct Fixture {
        api: Api,
        repository: Repository,
        artifacts: ArtifactService,
        owner: AuthenticatedAccount,
        other: AuthenticatedAccount,
        _root: TempDir,
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
        Fixture {
            api: Api::new(
                repository.clone(),
                artifacts.clone(),
                targets,
                pricing,
                || "2026-08-05T20:00:00Z".into(),
            ),
            repository,
            artifacts,
            owner: AuthenticatedAccount::from_verified_claim("account-a").unwrap(),
            other: AuthenticatedAccount::from_verified_claim("account-b").unwrap(),
            _root: root,
        }
    }

    fn request(operation_key: &str) -> Value {
        json!({
            "schema_version": 1,
            "operation_key": operation_key,
            "hubu_authorization_id": "auth-1",
            "hubu_claim_id": "claim-1",
            "hubu_token_reference": "sha256:opaque-reference",
            "authorization": {"amount_minor": 500, "currency": "USD"},
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
        assert_eq!(first.authorization.amount_minor, 500);

        // Object member ordering is immaterial to canonical immutable input.
        let mut reordered = request("operation-1");
        reordered["input"] = json!({
            "options": {"width": 512, "height": 512},
            "image_count": 1,
            "prompt": "cat"
        });
        let replay = execution(&call_create(&fixture, &reordered));
        assert_eq!(replay.execution_id, first.execution_id);

        let fetched = fixture.api.handle(
            "GET",
            &format!("/v1/executions/{}", first.execution_id),
            Some(&fixture.owner),
            &[],
        );
        assert_eq!(execution(&fetched), first);
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
            disabled_targets,
            changed_pricing,
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
                "unit_amount_minor":200}]
            }"#,
        )
        .unwrap();
        let changed_api = Api::new(
            fixture.repository.clone(),
            fixture.artifacts.clone(),
            changed_targets,
            changed_pricing,
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
        let api = Api::new(
            fixture.repository,
            fixture.artifacts,
            targets,
            pricing,
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
