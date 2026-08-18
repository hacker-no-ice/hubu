//! Gongbu-owned routes for the unified MCP adapter.
//!
//! This module intentionally mirrors Gongbu's public MCP wire contract without
//! depending on a Gongbu crate. Every HTTP request uses a fixed relative path
//! on the separately configured Gongbu client. Execution creation is sent once
//! and is never retried by the router.

use std::io::Read;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use reqwest::{header, Method, StatusCode};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::BackendClient;

const JSON_LIMIT: usize = 1024 * 1024;
const ARTIFACT_LIMIT: usize = 64 * 1024 * 1024;
const MCP_SCHEMA_VERSION: u32 = 2;

pub(crate) const TOOL_NAMES: &[&str] = &[
    "gongbu_create_execution",
    "gongbu_get_artifact",
    "gongbu_get_execution",
    "gongbu_list_artifacts",
];

pub(crate) fn tool_definitions() -> Vec<Value> {
    let create_properties = json!({
        "schema_version": {"type":"integer","const":2},
        "spend_auth_token_id": {"type":"string","minLength":1,"maxLength":255},
        "input": {"type":"object"},
        "input_schema_version": {"type":"integer","minimum":1},
        "workload_type": {"type":"string","minLength":1},
        "provider": {"type":"string","minLength":1},
        "adapter": {"type":"string","minLength":1},
        "model": {"type":"string","minLength":1}
    });
    vec![
        json!({"name":"gongbu_create_execution","description":"Create or replay a Gongbu execution from a Hubu spend authorization token and execution intent.","inputSchema":{"type":"object","additionalProperties":false,"required":["schema_version","spend_auth_token_id","input","input_schema_version","workload_type","provider","adapter","model"],"properties":create_properties}}),
        json!({"name":"gongbu_get_execution","description":"Get coarse status and redacted outcome for an execution.","inputSchema":id_schema("execution_id")}),
        json!({"name":"gongbu_list_artifacts","description":"List portable metadata for an execution's artifacts.","inputSchema":id_schema("execution_id")}),
        json!({"name":"gongbu_get_artifact","description":"Get portable base64 image content and safe metadata for an artifact.","inputSchema":id_schema("artifact_id")}),
    ]
}

fn id_schema(field: &str) -> Value {
    let mut properties = Map::new();
    properties.insert(
        field.into(),
        json!({"type":"string","minLength":1,"maxLength":255,"pattern":"^[A-Za-z0-9_-]+$"}),
    );
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": [field],
        "properties": properties
    })
}

pub(crate) fn call_tool(client: &BackendClient, name: &str, arguments: Value) -> Value {
    let result = match name {
        "gongbu_create_execution" => create(client, arguments),
        "gongbu_get_execution" => get_execution(client, arguments),
        "gongbu_list_artifacts" => list_artifacts(client, arguments),
        "gongbu_get_artifact" => get_artifact(client, arguments),
        _ => Err(ToolError::new("unknown_tool", "unknown Gongbu tool")),
    };
    serde_json::to_value(result.unwrap_or_else(ToolError::into_result))
        .expect("Gongbu MCP result serializes")
}

fn create(client: &BackendClient, arguments: Value) -> Result<ToolResult, ToolError> {
    let request: CreateExecutionRequest = parse_arguments(arguments)?;
    let response: ExecutionResponse =
        json_request(client, Method::POST, "v2/executions", Some(&request))?;
    Ok(text_result(&response))
}

fn get_execution(client: &BackendClient, arguments: Value) -> Result<ToolResult, ToolError> {
    let input: ExecutionIdInput = parse_arguments(arguments)?;
    validate_id(&input.execution_id)?;
    let response: ExecutionResponse = json_request::<Value, _>(
        client,
        Method::GET,
        &format!("v1/executions/{}", input.execution_id),
        None,
    )?;
    Ok(text_result(&response))
}

fn list_artifacts(client: &BackendClient, arguments: Value) -> Result<ToolResult, ToolError> {
    let input: ExecutionIdInput = parse_arguments(arguments)?;
    validate_id(&input.execution_id)?;
    let mut response: ArtifactListResponse = json_request::<Value, _>(
        client,
        Method::GET,
        &format!("v1/executions/{}/artifacts", input.execution_id),
        None,
    )?;
    for artifact in &mut response.artifacts {
        scrub_metadata(&mut artifact.metadata);
    }
    Ok(text_result(&response))
}

fn get_artifact(client: &BackendClient, arguments: Value) -> Result<ToolResult, ToolError> {
    let input: ArtifactIdInput = parse_arguments(arguments)?;
    validate_id(&input.artifact_id)?;
    let response = send(
        client,
        Method::GET,
        &format!("v1/artifacts/{}", input.artifact_id),
        None,
    )?;
    let status = response.status();
    if !status.is_success() {
        return Err(api_error(
            status,
            read_bounded(response, JSON_LIMIT).ok().as_deref(),
        ));
    }
    let media_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .filter(|value| matches!(*value, "image/png" | "image/jpeg"))
        .ok_or_else(|| {
            ToolError::upstream(
                "invalid_artifact",
                "Gongbu returned an unsupported artifact type",
            )
        })?
        .to_owned();
    let bytes = read_bounded(response, ARTIFACT_LIMIT).map_err(|()| {
        ToolError::upstream("invalid_artifact", "Gongbu returned an invalid artifact")
    })?;
    let metadata = json!({
        "schema_version": 1,
        "artifact_id": input.artifact_id,
        "media_type": media_type,
        "size_bytes": bytes.len(),
        "sha256": format!("sha256:{:x}", Sha256::digest(&bytes)),
        "encoding": "base64"
    });
    Ok(ToolResult {
        content: vec![
            Content::Text {
                text: serde_json::to_string(&metadata).expect("metadata serializes"),
            },
            Content::Image {
                data: BASE64.encode(bytes),
                mime_type: media_type,
            },
        ],
        is_error: false,
    })
}

fn json_request<B: Serialize + ?Sized, R: DeserializeOwned>(
    client: &BackendClient,
    method: Method,
    path: &str,
    body: Option<&B>,
) -> Result<R, ToolError> {
    let body = body
        .map(serde_json::to_vec)
        .transpose()
        .map_err(|_| ToolError::invalid())?;
    let response = send(client, method, path, body)?;
    let status = response.status();
    let bytes = read_bounded(response, JSON_LIMIT).map_err(|()| {
        ToolError::upstream("invalid_response", "Gongbu returned an invalid response")
    })?;
    if !status.is_success() {
        return Err(api_error(status, Some(&bytes)));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| ToolError::upstream("invalid_response", "Gongbu returned an invalid response"))
}

fn send(
    client: &BackendClient,
    method: Method,
    path: &str,
    body: Option<Vec<u8>>,
) -> Result<reqwest::blocking::Response, ToolError> {
    debug_assert!(!path.starts_with('/') && !path.contains("://"));
    let url = client
        .endpoint()
        .join(path)
        .map_err(|_| ToolError::transport())?;
    let mut request = client.http_client().request(method, url);
    if let Some(body) = body {
        request = request
            .header(header::CONTENT_TYPE, "application/json")
            .body(body);
    }
    // Deliberately no retries. An ambiguous create remains ambiguous and must
    // be recovered by replaying the same Hubu-issued authorization token.
    request.send().map_err(|_| ToolError::transport())
}

fn read_bounded(response: reqwest::blocking::Response, limit: usize) -> Result<Vec<u8>, ()> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(());
    }
    let mut bytes = Vec::new();
    response
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() > limit {
        Err(())
    } else {
        Ok(bytes)
    }
}

fn parse_arguments<T: DeserializeOwned>(arguments: Value) -> Result<T, ToolError> {
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

fn scrub_metadata(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.retain(|key, _| {
                let key = key.to_ascii_lowercase();
                !key.contains("storage_key")
                    && !key.contains("storage_path")
                    && key != "path"
                    && !key.ends_with("_path")
            });
            for (key, value) in object {
                let key = key.to_ascii_lowercase();
                if ["token", "secret", "credential", "authorization", "header"]
                    .iter()
                    .any(|needle| key.contains(needle))
                {
                    *value = Value::String("[REDACTED]".into());
                } else {
                    scrub_metadata(value);
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(scrub_metadata),
        _ => {}
    }
}

#[derive(Debug)]
struct ToolError {
    code: &'static str,
    message: &'static str,
}

impl ToolError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }

    fn invalid() -> Self {
        Self::new("invalid_request", "tool arguments failed validation")
    }

    fn transport() -> Self {
        Self::new("gongbu_unavailable", "Gongbu could not be reached")
    }

    fn upstream(code: &'static str, message: &'static str) -> Self {
        Self::new(code, message)
    }

    fn into_result(self) -> ToolResult {
        ToolResult {
            content: vec![Content::Text {
                text: json!({
                    "schema_version": MCP_SCHEMA_VERSION,
                    "error": { "code": self.code, "message": self.message }
                })
                .to_string(),
            }],
            is_error: true,
        }
    }
}

fn api_error(status: StatusCode, body: Option<&[u8]>) -> ToolError {
    #[derive(Deserialize)]
    struct Envelope {
        error: ErrorCode,
    }
    #[derive(Deserialize)]
    struct ErrorCode {
        code: String,
    }

    let reported = body
        .and_then(|bytes| serde_json::from_slice::<Envelope>(bytes).ok())
        .map(|value| value.error.code);
    match (status, reported.as_deref()) {
        (StatusCode::BAD_REQUEST, Some("invalid_request")) => {
            ToolError::new("invalid_request", "request validation failed")
        }
        (StatusCode::UNAUTHORIZED, _) => {
            ToolError::new("unauthorized", "Gongbu rejected operator authentication")
        }
        (StatusCode::FORBIDDEN, _) => ToolError::new("forbidden", "resource access is forbidden"),
        (StatusCode::NOT_FOUND, _) => ToolError::new("not_found", "resource was not found"),
        (StatusCode::CONFLICT, Some("immutable_scope_conflict")) => ToolError::new(
            "immutable_scope_conflict",
            "operation key was already used with different immutable input",
        ),
        (StatusCode::TOO_MANY_REQUESTS, _) => {
            ToolError::new("rate_limited", "Gongbu rate limit exceeded")
        }
        _ if status.is_server_error() => ToolError::new(
            "gongbu_internal_error",
            "Gongbu could not complete the request",
        ),
        _ => ToolError::new("gongbu_api_error", "Gongbu rejected the request"),
    }
}

fn text_result(value: &impl Serialize) -> ToolResult {
    ToolResult {
        content: vec![Content::Text {
            text: serde_json::to_string(value).expect("response serializes"),
        }],
        is_error: false,
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateExecutionRequest {
    schema_version: u32,
    spend_auth_token_id: String,
    input: Value,
    input_schema_version: i64,
    workload_type: String,
    provider: String,
    adapter: String,
    model: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Money {
    amount_minor: i64,
    currency: String,
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

#[derive(Debug, Deserialize, Serialize)]
struct ExecutionResponse {
    schema_version: u32,
    execution_id: String,
    operation_key: String,
    status: String,
    outcome: Option<String>,
    failure: Option<FailureResponse>,
    authorization: Money,
    created_at: String,
    updated_at: String,
    started_at: Option<String>,
    completed_at: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct FailureResponse {
    code: String,
    message: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct ArtifactListResponse {
    schema_version: u32,
    execution_id: String,
    artifacts: Vec<ArtifactResponse>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ArtifactResponse {
    artifact_id: String,
    execution_id: String,
    kind: String,
    media_type: String,
    size_bytes: i64,
    sha256: String,
    metadata: Value,
    metadata_schema_version: i64,
    created_at: String,
}

#[derive(Debug, Serialize)]
struct ToolResult {
    content: Vec<Content>,
    #[serde(rename = "isError")]
    is_error: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Content {
    Text {
        text: String,
    },
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}
