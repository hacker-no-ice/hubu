use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;

use super::response::ToolError;

pub(super) enum PreparedCall {
    Create(CreateExecutionRequest),
    GetExecution(String),
    ListArtifacts(String),
    GetArtifact(String),
}

pub(super) fn prepare(name: &str, arguments: Value) -> Result<PreparedCall, ToolError> {
    match name {
        "gongbu_create_execution" => Ok(PreparedCall::Create(parse(arguments)?)),
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
