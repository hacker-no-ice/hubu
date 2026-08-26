use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::AdmissionDiagnostic;

const MCP_SCHEMA_VERSION: u32 = 2;
pub(super) const EXECUTION_V1_SCHEMA_VERSION: u32 = 1;
pub(super) const EXECUTION_V2_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ApiErrorContext {
    General,
    CreateExecutionV2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ToolErrorClass {
    Permanent,
    Transient,
    InvalidSuccessfulResponse,
    IdentityConflict,
}

#[derive(Debug)]
pub(super) struct ToolError {
    code: &'static str,
    message: &'static str,
    diagnostic: Option<AdmissionDiagnostic>,
    class: ToolErrorClass,
}

impl ToolError {
    pub(super) fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message,
            diagnostic: None,
            class: ToolErrorClass::Permanent,
        }
    }

    fn with_diagnostic(mut self, diagnostic: Option<AdmissionDiagnostic>) -> Self {
        self.diagnostic = diagnostic;
        self
    }

    pub(super) fn invalid() -> Self {
        Self::new("invalid_request", "tool arguments failed validation")
    }

    pub(super) fn transport() -> Self {
        Self::transient("gongbu_unavailable", "Gongbu could not be reached")
    }

    pub(super) fn upstream(code: &'static str, message: &'static str) -> Self {
        Self::new(code, message)
    }

    pub(super) fn transient(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message,
            diagnostic: None,
            class: ToolErrorClass::Transient,
        }
    }

    pub(super) fn invalid_response() -> Self {
        Self {
            code: "invalid_response",
            message: "Gongbu returned an invalid response",
            diagnostic: None,
            class: ToolErrorClass::InvalidSuccessfulResponse,
        }
    }

    fn identity_conflict(message: &'static str) -> Self {
        Self {
            code: "identity_conflict",
            message,
            diagnostic: None,
            class: ToolErrorClass::IdentityConflict,
        }
    }

    pub(super) fn into_result(self) -> ToolResult {
        let mut error = json!({ "code": self.code, "message": self.message });
        if let Some(diagnostic) = self.diagnostic {
            error["reason_code"] = json!(diagnostic.reason_code());
            error["fields"] = json!(diagnostic.fields());
        }
        ToolResult {
            content: vec![Content::Text {
                text: json!({
                    "schema_version": MCP_SCHEMA_VERSION,
                    "error": error
                })
                .to_string(),
            }],
            is_error: true,
        }
    }

    pub(super) fn code(&self) -> &'static str {
        self.code
    }

    pub(super) fn class(&self) -> ToolErrorClass {
        self.class
    }

    pub(super) fn admission_diagnostic(&self) -> Option<AdmissionDiagnostic> {
        self.diagnostic
    }
}

pub(super) fn api_error(
    status: StatusCode,
    body: Option<&[u8]>,
    context: ApiErrorContext,
) -> ToolError {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        schema_version: Option<u32>,
        error: ErrorCode,
    }
    #[derive(Deserialize)]
    struct ErrorCode {
        code: String,
        #[serde(default)]
        reason_code: Option<Value>,
        #[serde(default)]
        fields: Option<Value>,
    }

    let reported = body.and_then(|bytes| serde_json::from_slice::<Envelope>(bytes).ok());
    match (
        status,
        reported.as_ref().map(|value| value.error.code.as_str()),
    ) {
        (StatusCode::BAD_REQUEST, Some("invalid_request")) => {
            let diagnostic = reported
                .as_ref()
                .filter(|reported| {
                    context == ApiErrorContext::CreateExecutionV2
                        && reported.schema_version == Some(2)
                })
                .and_then(|reported| {
                    allowlisted_validation_diagnostic(
                        reported.error.reason_code.as_ref(),
                        reported.error.fields.as_ref(),
                    )
                });
            ToolError::new("invalid_request", "request validation failed")
                .with_diagnostic(diagnostic)
        }
        (StatusCode::UNAUTHORIZED, _) => {
            ToolError::new("unauthorized", "Gongbu rejected operator authentication")
        }
        (StatusCode::FORBIDDEN, _) => ToolError::new("forbidden", "resource access is forbidden"),
        (StatusCode::NOT_FOUND, _) => ToolError::new("not_found", "resource was not found"),
        (StatusCode::CONFLICT, Some("immutable_scope_conflict")) => ToolError::new(
            "immutable_scope_conflict",
            "authorization continuation was already used with different immutable input",
        ),
        (StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_EARLY, _) => ToolError::transient(
            "gongbu_unavailable",
            "Gongbu could not complete the request",
        ),
        (StatusCode::TOO_MANY_REQUESTS, _) => {
            ToolError::transient("rate_limited", "Gongbu rate limit exceeded")
        }
        _ if status.is_server_error() => ToolError::transient(
            "gongbu_internal_error",
            "Gongbu could not complete the request",
        ),
        _ => ToolError::new("gongbu_api_error", "Gongbu rejected the request"),
    }
}

fn allowlisted_validation_diagnostic(
    reason_code: Option<&Value>,
    fields: Option<&Value>,
) -> Option<AdmissionDiagnostic> {
    let reason_code = reason_code?.as_str()?;
    let fields = fields?.as_array()?;
    let exactly_matches = |expected: &[&str]| {
        fields.len() == expected.len()
            && fields
                .iter()
                .zip(expected)
                .all(|(reported, expected)| reported.as_str() == Some(*expected))
    };
    match reason_code {
        "target_not_selectable"
            if exactly_matches(AdmissionDiagnostic::TargetNotSelectable.fields()) =>
        {
            Some(AdmissionDiagnostic::TargetNotSelectable)
        }
        "pricing_selector_not_matched"
            if exactly_matches(AdmissionDiagnostic::PricingSelectorNotMatched.fields()) =>
        {
            Some(AdmissionDiagnostic::PricingSelectorNotMatched)
        }
        _ => None,
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

pub(super) fn execution_result(
    response: ExecutionResponse,
    expected: Option<&crate::operation_registry::GongbuContinuation>,
    requested_execution_id: Option<&str>,
    expected_schema_version: u32,
) -> Result<(ToolResult, crate::operation_registry::GongbuLifecycle), ToolError> {
    if expected.is_some_and(|expected| expected.operation_key != response.operation_key) {
        return Err(ToolError::identity_conflict(
            "Gongbu returned an execution for a different normalized operation",
        ));
    }
    if requested_execution_id.is_some_and(|execution_id| execution_id != response.execution_id)
        || expected
            .and_then(|expected| expected.execution_id.as_deref())
            .is_some_and(|execution_id| execution_id != response.execution_id)
    {
        return Err(ToolError::identity_conflict(
            "Gongbu returned a conflicting execution identity",
        ));
    }
    if response.schema_version != expected_schema_version
        || !valid_execution_id(&response.execution_id)
        || !valid_execution_status(&response.status)
    {
        return Err(ToolError::invalid_response());
    }
    let lifecycle = crate::operation_registry::GongbuLifecycle {
        execution_id: response.execution_id.clone(),
        operation_key: response.operation_key.clone(),
        status: response.status.clone(),
        outcome: response.outcome.clone(),
    };
    let private_operation_key = response.operation_key.clone();
    let public = PublicExecutionResponse {
        schema_version: response.schema_version,
        execution_id: response.execution_id,
        operation_handle: expected.map(|expected| expected.operation_handle.clone()),
        status: response.status,
        outcome: response.outcome,
        failure: response.failure,
        authorization: response.authorization,
        created_at: response.created_at,
        updated_at: response.updated_at,
        started_at: response.started_at,
        completed_at: response.completed_at,
    };
    let mut public = serde_json::to_value(public).expect("public execution response serializes");
    scrub_private_projection(&mut public, &private_operation_key);
    Ok((text_result(&public), lifecycle))
}

fn valid_execution_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_execution_status(status: &str) -> bool {
    matches!(
        status,
        "pending"
            | "preflighting"
            | "claimed"
            | "executing"
            | "settling"
            | "succeeded"
            | "released"
            | "failed"
            | "reconciliation_required"
    )
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
                    && key != "operation_key"
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
        Value::String(text) => scrub_private_text(text, None),
        _ => {}
    }
}

fn scrub_private_text(text: &mut String, private_operation_key: Option<&str>) {
    if let Some(operation_key) = private_operation_key {
        *text = text.replace(operation_key, "<private operation redacted>");
    }
    if text.contains("hubu:operation:v1:") {
        *text = "<private operation redacted>".into();
    }
}

fn scrub_private_projection(value: &mut Value, private_operation_key: &str) {
    match value {
        Value::Object(object) => {
            object.retain(|key, _| key != "operation_key");
            object
                .values_mut()
                .for_each(|value| scrub_private_projection(value, private_operation_key));
        }
        Value::Array(values) => values
            .iter_mut()
            .for_each(|value| scrub_private_projection(value, private_operation_key)),
        Value::String(text) => scrub_private_text(text, Some(private_operation_key)),
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

#[derive(Debug, Serialize)]
struct PublicExecutionResponse {
    schema_version: u32,
    execution_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_handle: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation_registry::GongbuContinuation;

    fn execution() -> ExecutionResponse {
        ExecutionResponse {
            schema_version: EXECUTION_V2_SCHEMA_VERSION,
            execution_id: "exec-1".into(),
            operation_key: "operation-1".into(),
            status: "executing".into(),
            outcome: None,
            failure: None,
            authorization: Money {
                amount_minor: 25,
                currency: "USD".into(),
            },
            created_at: "now".into(),
            updated_at: "now".into(),
            started_at: None,
            completed_at: None,
        }
    }

    fn continuation(execution_id: Option<&str>) -> GongbuContinuation {
        GongbuContinuation {
            operation_key: "operation-1".into(),
            operation_handle: "hubu:public-operation:v1:test".into(),
            execution_id: execution_id.map(str::to_owned),
        }
    }

    fn result_error(
        result: Result<(ToolResult, crate::operation_registry::GongbuLifecycle), ToolError>,
    ) -> ToolError {
        match result {
            Ok(_) => panic!("execution response unexpectedly passed validation"),
            Err(error) => error,
        }
    }

    #[test]
    fn semantic_execution_response_failures_are_classified_before_persistence() {
        let mut wrong_schema = execution();
        wrong_schema.schema_version = 99;
        let mut invalid_id = execution();
        invalid_id.execution_id = "bad/execution".into();
        let mut unknown_status = execution();
        unknown_status.status = "future_status".into();

        for response in [wrong_schema, invalid_id, unknown_status] {
            let error = result_error(execution_result(
                response,
                Some(&continuation(None)),
                None,
                EXECUTION_V2_SCHEMA_VERSION,
            ));
            assert_eq!(error.code(), "invalid_response");
            assert_eq!(error.class(), ToolErrorClass::InvalidSuccessfulResponse);
        }
    }

    #[test]
    fn execution_identity_conflicts_remain_permanent_and_fail_closed() {
        let mut wrong_operation = execution();
        wrong_operation.operation_key = "another-operation".into();
        let error = result_error(execution_result(
            wrong_operation,
            Some(&continuation(None)),
            None,
            EXECUTION_V2_SCHEMA_VERSION,
        ));
        assert_eq!(error.code(), "identity_conflict");
        assert_eq!(error.class(), ToolErrorClass::IdentityConflict);

        let mut wrong_execution = execution();
        wrong_execution.schema_version = EXECUTION_V1_SCHEMA_VERSION;
        wrong_execution.execution_id = "another-execution".into();
        let error = result_error(execution_result(
            wrong_execution,
            Some(&continuation(Some("exec-1"))),
            Some("exec-1"),
            EXECUTION_V1_SCHEMA_VERSION,
        ));
        assert_eq!(error.code(), "identity_conflict");
        assert_eq!(error.class(), ToolErrorClass::IdentityConflict);
    }

    #[test]
    fn additive_execution_response_fields_remain_forward_compatible() {
        let mut value = serde_json::to_value(execution()).unwrap();
        value["future_additive_field"] = json!({"safe":true});
        let response: ExecutionResponse = serde_json::from_value(value).unwrap();
        let (_, lifecycle) = execution_result(
            response,
            Some(&continuation(None)),
            None,
            EXECUTION_V2_SCHEMA_VERSION,
        )
        .unwrap();
        assert_eq!(lifecycle.execution_id, "exec-1");
        assert_eq!(lifecycle.status, "executing");
    }
}
