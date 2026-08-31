use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;

use super::response::ToolError;

pub(super) enum PreparedCall {
    Create(CreateExecutionRequest),
    GetProviderCatalog,
    GetExecution(String),
    ListArtifacts(String),
    GetArtifact(String),
}

pub(super) fn prepare(name: &str, arguments: Value) -> Result<PreparedCall, ToolError> {
    match name {
        "gongbu_create_execution" => {
            reject_protected_overrides(&arguments)?;
            let request: CreateExecutionRequest = parse(arguments)?;
            request.validate()?;
            validate_id(&request.spend_auth_token_id)?;
            Ok(PreparedCall::Create(request))
        }
        "gongbu_get_provider_catalog" => {
            let _: EmptyInput = parse(arguments)?;
            Ok(PreparedCall::GetProviderCatalog)
        }
        "gongbu_get_execution" => {
            let input: ExecutionIdInput = parse(arguments)?;
            validate_id(&input.execution_id)?;
            Ok(PreparedCall::GetExecution(input.execution_id))
        }
        "gongbu_list_artifacts" => {
            let input: ExecutionIdInput = parse(arguments)?;
            validate_id(&input.execution_id)?;
            Ok(PreparedCall::ListArtifacts(input.execution_id))
        }
        "gongbu_get_artifact" => {
            let input: ArtifactIdInput = parse(arguments)?;
            validate_id(&input.artifact_id)?;
            Ok(PreparedCall::GetArtifact(input.artifact_id))
        }
        _ => Err(ToolError::new("unknown_tool", "unknown Gongbu tool")),
    }
}

pub(super) fn create_continuation_id(arguments: &Value) -> Result<String, ToolError> {
    reject_protected_overrides(arguments)?;
    let request: CreateExecutionRequest = parse(arguments.clone())?;
    request.validate()?;
    validate_id(&request.spend_auth_token_id)?;
    Ok(request.spend_auth_token_id)
}

pub(super) fn status_execution_id(arguments: &Value) -> Result<String, ToolError> {
    let input: ExecutionIdInput = parse(arguments.clone())?;
    validate_id(&input.execution_id)?;
    Ok(input.execution_id)
}

fn parse<T: DeserializeOwned>(arguments: Value) -> Result<T, ToolError> {
    serde_json::from_value(arguments).map_err(|_| ToolError::invalid())
}

fn validate_id(value: &str) -> Result<(), ToolError> {
    if value.is_empty()
        || value.len() > 255
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Err(ToolError::invalid())
    } else {
        Ok(())
    }
}

fn reject_protected_overrides(arguments: &Value) -> Result<(), ToolError> {
    let input = arguments.get("input").ok_or_else(ToolError::invalid)?;
    if contains_protected_override(input) {
        return Err(ToolError::new(
            "protected_override",
            "execution input contains protected platform or transport controls",
        ));
    }
    Ok(())
}

fn contains_protected_override(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().any(contains_protected_override),
        Value::Object(object) => object.iter().any(|(key, value)| {
            let normalized = key.to_ascii_lowercase().replace('-', "_");
            matches!(
                normalized.as_str(),
                "operation_key"
                    | "operation_handle"
                    | "task_id"
                    | "auth_token_id"
                    | "spend_auth_token_id"
                    | "decision_id"
                    | "claim_id"
                    | "hubu_claim_id"
                    | "execution_id"
                    | "execution_status"
                    | "continuation_state"
                    | "lifecycle_state"
                    | "gongbu_status"
                    | "endpoint"
                    | "base_url"
                    | "api_key"
                    | "credential"
                    | "credentials"
                    | "authorization_header"
                    | "bearer_token"
                    | "header"
                    | "headers"
                    | "retry"
                    | "retries"
                    | "max_retries"
                    | "retry_delay"
                    | "retry_backoff"
            ) || contains_protected_override(value)
        }),
        _ => false,
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateExecutionRequest {
    schema_version: u32,
    spend_auth_token_id: String,
    input: Value,
    input_schema_version: i64,
    workload_type: String,
    provider: String,
    adapter: String,
    model: String,
}

impl CreateExecutionRequest {
    fn validate(&self) -> Result<(), ToolError> {
        if self.schema_version != 2
            || !self.input.is_object()
            || self.input_schema_version < 1
            || [
                self.workload_type.as_str(),
                self.provider.as_str(),
                self.adapter.as_str(),
                self.model.as_str(),
            ]
            .into_iter()
            .any(str::is_empty)
        {
            return Err(ToolError::invalid());
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutionIdInput {
    execution_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactIdInput {
    artifact_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyInput {}
