use std::io::Read;

use reqwest::{header, Method};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;

use crate::BackendClient;

use super::{
    request::{self, PreparedCall},
    response::{
        api_error, artifact_result, scrub_artifact_metadata, text_result, ArtifactListResponse,
        ExecutionResponse, ToolError, ToolResult,
    },
};

const JSON_LIMIT: usize = 1024 * 1024;
const ARTIFACT_LIMIT: usize = 64 * 1024 * 1024;

pub(super) fn call_tool(client: &BackendClient, name: &str, arguments: Value) -> Value {
    let result = request::prepare(name, arguments).and_then(|call| execute(client, call));
    serde_json::to_value(result.unwrap_or_else(ToolError::into_result))
        .expect("Gongbu MCP result serializes")
}

fn execute(client: &BackendClient, call: PreparedCall) -> Result<ToolResult, ToolError> {
    match call {
        PreparedCall::Create(request) => {
            let response: ExecutionResponse =
                json_request(client, Method::POST, "v2/executions", Some(&request))?;
            Ok(text_result(&response))
        }
        PreparedCall::GetExecution(execution_id) => {
            let response: ExecutionResponse = json_request::<Value, _>(
                client,
                Method::GET,
                &format!("v1/executions/{execution_id}"),
                None,
            )?;
            Ok(text_result(&response))
        }
        PreparedCall::ListArtifacts(execution_id) => {
            let mut response: ArtifactListResponse = json_request::<Value, _>(
                client,
                Method::GET,
                &format!("v1/executions/{execution_id}/artifacts"),
                None,
            )?;
            scrub_artifact_metadata(&mut response);
            Ok(text_result(&response))
        }
        PreparedCall::GetArtifact(artifact_id) => get_artifact(client, artifact_id),
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
