use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const MCP_SCHEMA_VERSION: u32 = 2;

#[derive(Debug)]
pub(super) struct ToolError {
    code: &'static str,
    message: &'static str,
}

impl ToolError {
    pub(super) fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }

    pub(super) fn invalid() -> Self {
        Self::new("invalid_request", "tool arguments failed validation")
    }

    pub(super) fn transport() -> Self {
        Self::new("gongbu_unavailable", "Gongbu could not be reached")
    }

    pub(super) fn upstream(code: &'static str, message: &'static str) -> Self {
        Self::new(code, message)
    }

    pub(super) fn into_result(self) -> ToolResult {
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

pub(super) fn api_error(status: StatusCode, body: Option<&[u8]>) -> ToolError {
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

pub(super) fn text_result(value: &impl Serialize) -> ToolResult {
    ToolResult {
        content: vec![Content::Text {
            text: serde_json::to_string(value).expect("response serializes"),
        }],
        is_error: false,
    }
}

pub(super) fn artifact_result(
    artifact_id: String,
    media_type: String,
    bytes: Vec<u8>,
) -> ToolResult {
    let metadata = json!({
        "schema_version": 1,
        "artifact_id": artifact_id,
        "media_type": media_type,
        "size_bytes": bytes.len(),
        "sha256": format!("sha256:{:x}", Sha256::digest(&bytes)),
        "encoding": "base64"
    });
    ToolResult {
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
    }
}

pub(super) fn scrub_artifact_metadata(response: &mut ArtifactListResponse) {
    for artifact in &mut response.artifacts {
        scrub_metadata(&mut artifact.metadata);
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

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Money {
    amount_minor: i64,
    currency: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct ExecutionResponse {
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
pub(super) struct ArtifactListResponse {
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
pub(super) struct ToolResult {
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
