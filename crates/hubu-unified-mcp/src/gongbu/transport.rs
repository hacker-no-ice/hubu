use std::io::Read;

use reqwest::{header, Method};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;

use crate::BackendClient;

use super::{
    request::{self, PreparedCall},
    response::{
        api_error, artifact_result, execution_result, scrub_artifact_metadata, text_result,
        ApiErrorContext, ArtifactListResponse, ExecutionResponse, ToolError, ToolResult,
    },
};

const JSON_LIMIT: usize = 1024 * 1024;
const ARTIFACT_LIMIT: usize = 64 * 1024 * 1024;

pub(crate) struct CallOutcome {
    pub(crate) result: Value,
    pub(crate) lifecycle: Option<crate::operation_registry::GongbuLifecycle>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DurableCallError {
    pub(crate) code: &'static str,
    pub(crate) retryable: bool,
}

impl From<ToolError> for DurableCallError {
    fn from(error: ToolError) -> Self {
        Self {
            code: error.code(),
            retryable: error.retryable(),
        }
    }
}

pub(super) fn call_tool(
    client: &BackendClient,
    name: &str,
    arguments: Value,
    expected: Option<&crate::operation_registry::GongbuContinuation>,
) -> CallOutcome {
    match request::prepare(name, arguments).and_then(|call| execute(client, call, expected)) {
        Ok((result, lifecycle)) => CallOutcome {
            result: serde_json::to_value(result).expect("Gongbu MCP result serializes"),
            lifecycle,
        },
        Err(error) => CallOutcome {
            result: serde_json::to_value(error.into_result()).expect("Gongbu MCP error serializes"),
            lifecycle: None,
        },
    }
}

pub(super) fn create_durable_execution(
    client: &BackendClient,
    arguments: Value,
    expected: &crate::operation_registry::GongbuContinuation,
) -> Result<crate::operation_registry::GongbuLifecycle, DurableCallError> {
    let prepared = request::prepare("gongbu_create_execution", arguments)?;
    let (_, lifecycle) = execute(client, prepared, Some(expected)).map_err(|error| {
        let code = error.code();
        DurableCallError {
            code,
            // A successful create response that cannot be read or decoded is
            // ambiguous: Gongbu may already have persisted the execution. The
            // exact operation-key-bound request is safe to replay.
            retryable: error.retryable() || code == "invalid_response",
        }
    })?;
    lifecycle.ok_or(DurableCallError {
        code: "invalid_execution_response",
        retryable: false,
    })
}

pub(super) fn observe_durable_execution(
    client: &BackendClient,
    execution_id: &str,
    expected: &crate::operation_registry::GongbuContinuation,
) -> Result<crate::operation_registry::GongbuLifecycle, DurableCallError> {
    let prepared = request::prepare(
        "gongbu_get_execution",
        serde_json::json!({"execution_id": execution_id}),
    )?;
    let (_, lifecycle) = execute(client, prepared, Some(expected))?;
    lifecycle.ok_or(DurableCallError {
        code: "invalid_execution_response",
        retryable: false,
    })
}

fn execute(
    client: &BackendClient,
    call: PreparedCall,
    expected: Option<&crate::operation_registry::GongbuContinuation>,
) -> Result<
    (
        ToolResult,
        Option<crate::operation_registry::GongbuLifecycle>,
    ),
    ToolError,
> {
    match call {
        PreparedCall::Create(request) => {
            let expected = expected.ok_or_else(|| {
                ToolError::new(
                    "unknown_continuation",
                    "Gongbu execution requires a bound authorization continuation",
                )
            })?;
            let response: ExecutionResponse =
                json_request(client, Method::POST, "v2/executions", Some(&request))?;
            let (result, lifecycle) = execution_result(response, Some(expected))?;
            Ok((result, Some(lifecycle)))
        }
        PreparedCall::GetExecution(execution_id) => {
            let response: ExecutionResponse = json_request::<Value, _>(
                client,
                Method::GET,
                &format!("v1/executions/{execution_id}"),
                None,
            )?;
            let (result, lifecycle) = execution_result(response, expected)?;
            Ok((result, expected.map(|_| lifecycle)))
        }
        PreparedCall::ListArtifacts(execution_id) => {
            let mut response: ArtifactListResponse = json_request::<Value, _>(
                client,
                Method::GET,
                &format!("v1/executions/{execution_id}/artifacts"),
                None,
            )?;
            scrub_artifact_metadata(&mut response);
            Ok((text_result(&response), None))
        }
        PreparedCall::GetArtifact(artifact_id) => {
            get_artifact(client, artifact_id).map(|result| (result, None))
        }
    }
}

fn get_artifact(client: &BackendClient, artifact_id: String) -> Result<ToolResult, ToolError> {
    let response = send(
        client,
        Method::GET,
        &format!("v1/artifacts/{artifact_id}"),
        None,
    )?;
    let status = response.status();
    if !status.is_success() {
        return Err(api_error(
            status,
            read_bounded(response, JSON_LIMIT).ok().as_deref(),
            ApiErrorContext::General,
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
    Ok(artifact_result(artifact_id, media_type, bytes))
}

fn json_request<B: Serialize + ?Sized, R: DeserializeOwned>(
    client: &BackendClient,
    method: Method,
    path: &str,
    body: Option<&B>,
) -> Result<R, ToolError> {
    let error_context = if method == Method::POST && path == "v2/executions" {
        ApiErrorContext::CreateExecutionV2
    } else {
        ApiErrorContext::General
    };
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
        return Err(api_error(status, Some(&bytes), error_context));
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
