//! MCP adapter for Gongbu's authenticated v1 HTTP API.
//!
//! This crate deliberately has no dependency on `gongbu-api`. Replay,
//! authorization, pricing, execution, and persistence remain service concerns.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use reqwest::{blocking::Client, header, StatusCode, Url};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{env, io::Read, time::Duration};
use thiserror::Error;

const JSON_LIMIT: usize = 1024 * 1024;
const ARTIFACT_LIMIT: usize = 64 * 1024 * 1024;
const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 2_000;
const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 30_000;
const MAX_TIMEOUT_MS: u64 = 300_000;

#[derive(Clone)]
pub struct Config {
    endpoint: Url,
    bearer_token: String,
    connect_timeout: Duration,
    request_timeout: Duration,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::new(
            &required_env("GONGBU_MCP_ENDPOINT")?,
            required_env("GONGBU_MCP_BEARER_TOKEN")?,
            env_timeout("GONGBU_MCP_CONNECT_TIMEOUT_MS", DEFAULT_CONNECT_TIMEOUT_MS)?,
            env_timeout("GONGBU_MCP_REQUEST_TIMEOUT_MS", DEFAULT_REQUEST_TIMEOUT_MS)?,
        )
    }

    pub fn new(
        endpoint: &str,
        bearer_token: String,
        connect_timeout: Duration,
        request_timeout: Duration,
    ) -> Result<Self, ConfigError> {
        let mut endpoint = Url::parse(endpoint).map_err(|_| ConfigError::Endpoint)?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || endpoint.cannot_be_a_base()
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(ConfigError::Endpoint);
        }
        if bearer_token.trim().is_empty() {
            return Err(ConfigError::Missing("GONGBU_MCP_BEARER_TOKEN"));
        }
        validate_timeout(connect_timeout)?;
        validate_timeout(request_timeout)?;
        if !endpoint.path().ends_with('/') {
            endpoint.set_path(&format!("{}/", endpoint.path()));
        }
        Ok(Self {
            endpoint,
            bearer_token,
            connect_timeout,
            request_timeout,
        })
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("required operator configuration {0} is missing")]
    Missing(&'static str),
    #[error(
        "GONGBU_MCP_ENDPOINT must be an HTTP(S) base URL without credentials, query, or fragment"
    )]
    Endpoint,
    #[error("configured timeout must be between 1 and 300000 milliseconds")]
    Timeout,
}

fn required_env(name: &'static str) -> Result<String, ConfigError> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or(ConfigError::Missing(name))
}

fn env_timeout(name: &'static str, default: u64) -> Result<Duration, ConfigError> {
    let millis = match env::var(name) {
        Ok(value) => value.parse().map_err(|_| ConfigError::Timeout)?,
        Err(_) => default,
    };
    let timeout = Duration::from_millis(millis);
    validate_timeout(timeout)?;
    Ok(timeout)
}

fn validate_timeout(timeout: Duration) -> Result<(), ConfigError> {
    if timeout.is_zero() || timeout > Duration::from_millis(MAX_TIMEOUT_MS) {
        Err(ConfigError::Timeout)
    } else {
        Ok(())
    }
}

#[derive(Clone)]
pub struct GongbuClient {
    config: Config,
    client: Client,
}

impl GongbuClient {
    pub fn new(config: Config) -> Result<Self, ConfigError> {
        let mut headers = header::HeaderMap::new();
        let authorization =
            header::HeaderValue::from_str(&format!("Bearer {}", config.bearer_token))
                .map_err(|_| ConfigError::Missing("GONGBU_MCP_BEARER_TOKEN"))?;
        headers.insert(header::AUTHORIZATION, authorization);
        let client = Client::builder()
            .default_headers(headers)
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| ConfigError::Endpoint)?;
        Ok(Self { config, client })
    }

    pub fn call_tool(&self, name: &str, arguments: Value) -> ToolResult {
        let result = match name {
            "gongbu_preview_authorization_scope" => self.preview_authorization_scope(arguments),
            "gongbu_create_execution" => self.create(arguments),
            "gongbu_get_execution" => self.get_execution(arguments),
            "gongbu_list_artifacts" => self.list_artifacts(arguments),
            "gongbu_get_artifact" => self.get_artifact(arguments),
            _ => Err(ToolError::new("unknown_tool", "unknown Gongbu tool")),
        };
        result.unwrap_or_else(ToolError::into_result)
    }

    fn preview_authorization_scope(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let request: AuthorizationScopePreviewRequest = parse_arguments(arguments)?;
        let response: Value = self.json_request(
            reqwest::Method::POST,
            "v1/authorization-scopes/preview",
            Some(&request),
        )?;
        Ok(text_result(&response))
    }

    fn create(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let request: CreateExecutionRequest = parse_arguments(arguments)?;
        let response: ExecutionResponse =
            self.json_request(reqwest::Method::POST, "v1/executions", Some(&request))?;
        Ok(text_result(&response))
    }

    fn get_execution(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let input: ExecutionIdInput = parse_arguments(arguments)?;
        validate_id(&input.execution_id)?;
        let response: ExecutionResponse = self.json_request::<Value, _>(
            reqwest::Method::GET,
            &format!("v1/executions/{}", input.execution_id),
            None,
        )?;
        Ok(text_result(&response))
    }

    fn list_artifacts(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let input: ExecutionIdInput = parse_arguments(arguments)?;
        validate_id(&input.execution_id)?;
        let mut response: ArtifactListResponse = self.json_request::<Value, _>(
            reqwest::Method::GET,
            &format!("v1/executions/{}/artifacts", input.execution_id),
            None,
        )?;
        for artifact in &mut response.artifacts {
            scrub_metadata(&mut artifact.metadata);
        }
        Ok(text_result(&response))
    }

    fn get_artifact(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let input: ArtifactIdInput = parse_arguments(arguments)?;
        validate_id(&input.artifact_id)?;
        let response = self.send(
            reqwest::Method::GET,
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
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<R, ToolError> {
        let body = body
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|_| ToolError::invalid())?;
        let response = self.send(method, path, body)?;
        let status = response.status();
        let bytes = read_bounded(response, JSON_LIMIT).map_err(|()| {
            ToolError::upstream("invalid_response", "Gongbu returned an invalid response")
        })?;
        if !status.is_success() {
            return Err(api_error(status, Some(&bytes)));
        }
        serde_json::from_slice(&bytes).map_err(|_| {
            ToolError::upstream("invalid_response", "Gongbu returned an invalid response")
        })
    }

    fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> Result<reqwest::blocking::Response, ToolError> {
        let url = self
            .config
            .endpoint
            .join(path)
            .map_err(|_| ToolError::transport())?;
        let mut request = self.client.request(method, url);
        if let Some(body) = body {
            request = request
                .header(header::CONTENT_TYPE, "application/json")
                .body(body);
        }
        // Deliberately no retries. Execution creation is sent exactly once.
        request.send().map_err(|_| ToolError::transport())
    }
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
                    "schema_version": 1,
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
    operation_key: String,
    hubu_authorization_id: String,
    hubu_claim_id: Option<String>,
    hubu_token_reference: String,
    authorization: Money,
    input: Value,
    input_schema_version: i64,
    workload_type: String,
    provider: String,
    #[serde(default)]
    execution_scope: Option<Value>,
    adapter: String,
    model: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AuthorizationScopePreviewRequest {
    schema_version: u32,
    operation_key: String,
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
pub struct ToolResult {
    pub content: Vec<Content>,
    #[serde(rename = "isError")]
    pub is_error: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Content {
    Text {
        text: String,
    },
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

pub fn tool_definitions() -> Value {
    let create_properties = json!({
        "schema_version": {"type":"integer","const":1},
        "operation_key": {"type":"string","minLength":1,"maxLength":255},
        "hubu_authorization_id": {"type":"string","minLength":1},
        "hubu_claim_id": {"type":["string","null"]},
        "hubu_token_reference": {"type":"string","minLength":1},
        "authorization": {"type":"object","additionalProperties":false,"required":["amount_minor","currency"],"properties":{"amount_minor":{"type":"integer","minimum":0},"currency":{"type":"string","pattern":"^[A-Za-z]{3}$"}}},
        "input": {"type":"object"},
        "input_schema_version": {"type":"integer","minimum":1},
        "workload_type": {"type":"string","minLength":1},
        "provider": {"type":"string","minLength":1},
        "execution_scope": {"type":"object","additionalProperties":false,"required":["schema_version","provider","executor","capability","billing_merchant"]},
        "adapter": {"type":"string","minLength":1},
        "model": {"type":"string","minLength":1}
    });
    json!([
        {"name":"gongbu_preview_authorization_scope","description":"Derive the exact operator-owned Hubu authorization request for a planned Gongbu execution before token issuance.","inputSchema":{"type":"object","additionalProperties":false,"required":["schema_version","operation_key","input","input_schema_version","workload_type","provider","adapter","model"],"properties":{
            "schema_version":{"type":"integer","const":1},
            "operation_key":{"type":"string","minLength":1,"maxLength":255},
            "input":{"type":"object"},
            "input_schema_version":{"type":"integer","minimum":1},
            "workload_type":{"type":"string","minLength":1},
            "provider":{"type":"string","minLength":1},
            "adapter":{"type":"string","minLength":1},
            "model":{"type":"string","minLength":1}
        }}},
        {"name":"gongbu_create_execution","description":"Create or replay a Gongbu execution using an existing Hubu authorization.","inputSchema":{"type":"object","additionalProperties":false,"required":["schema_version","operation_key","hubu_authorization_id","hubu_token_reference","authorization","input","input_schema_version","workload_type","provider","adapter","model"],"properties":create_properties}},
        {"name":"gongbu_get_execution","description":"Get coarse status and redacted outcome for an execution.","inputSchema":id_schema("execution_id")},
        {"name":"gongbu_list_artifacts","description":"List portable metadata for an execution's artifacts.","inputSchema":id_schema("execution_id")},
        {"name":"gongbu_get_artifact","description":"Get portable base64 image content and safe metadata for an artifact.","inputSchema":id_schema("artifact_id")}
    ])
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::{TcpListener, TcpStream},
        sync::{Arc, Mutex},
        thread,
    };

    fn mock_server(
        responses: Vec<(&'static str, &'static str, &'static str)>,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        thread::spawn(move || {
            for (status, content_type, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_request(&mut stream);
                captured.lock().unwrap().push(request);
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        (address, requests)
    }

    fn read_request(stream: &mut TcpStream) -> String {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request = String::new();
        let mut content_length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                content_length = value.trim().parse().unwrap();
            }
            request.push_str(&line);
        }
        let mut body = vec![0; content_length];
        reader.read_exact(&mut body).unwrap();
        request.push_str(&String::from_utf8(body).unwrap());
        request
    }

    fn client(endpoint: &str, token: &str) -> GongbuClient {
        GongbuClient::new(
            Config::new(
                endpoint,
                token.into(),
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn create_arguments() -> Value {
        json!({"schema_version":1,"operation_key":"op-1","hubu_authorization_id":"auth-1","hubu_claim_id":null,"hubu_token_reference":"hubu-ref-1","authorization":{"amount_minor":25,"currency":"USD"},"input":{"prompt":"circle","image_count":1},"input_schema_version":1,"workload_type":"image_generation","provider":"example","adapter":"fixture","model":"v1"})
    }

    fn preview_arguments() -> Value {
        json!({"schema_version":1,"operation_key":"op-1","input":{"prompt":"circle","image_count":1},"input_schema_version":1,"workload_type":"image_generation","provider":"example","adapter":"fixture","model":"v1"})
    }

    const EXECUTION: &str = r#"{"schema_version":1,"execution_id":"exec-1","operation_key":"op-1","status":"pending","outcome":null,"failure":null,"authorization":{"amount_minor":25,"currency":"USD"},"created_at":"now","updated_at":"now","started_at":null,"completed_at":null}"#;

    #[test]
    fn authorization_preview_is_forwarded_without_operator_owned_overrides() {
        let response = r#"{"authorization_scope":{"schema_version":1,"account_id":"operator-account"},"hubu_authorize_request":{"operation_key":"op-1"}}"#;
        let (endpoint, requests) = mock_server(vec![("200 OK", "application/json", response)]);
        let result = client(&endpoint, "operator-secret")
            .call_tool("gongbu_preview_authorization_scope", preview_arguments());
        assert!(!result.is_error);
        let requests = requests.lock().unwrap();
        assert!(requests[0].contains("POST /v1/authorization-scopes/preview"));
        assert!(!requests[0].contains("account_id") && !requests[0].contains("agent_id"));
    }

    #[test]
    fn create_replay_is_forwarded_once_per_call_and_returns_stable_id() {
        let (endpoint, requests) = mock_server(vec![
            ("200 OK", "application/json", EXECUTION),
            ("200 OK", "application/json", EXECUTION),
        ]);
        let client = client(&endpoint, "operator-secret");
        let first = client.call_tool("gongbu_create_execution", create_arguments());
        let second = client.call_tool("gongbu_create_execution", create_arguments());
        assert!(!first.is_error && !second.is_error);
        assert!(format!("{:?}", first.content).contains("exec-1"));
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests.iter().all(|request| {
            let request = request.to_ascii_lowercase();
            request.matches("post /v1/executions").count() == 1
                && request.contains("authorization: bearer operator-secret")
                && !request.contains("account_id")
        }));
    }

    #[test]
    fn immutable_conflict_and_cross_account_errors_are_stable_and_redacted() {
        let secret = "operator-token-canary";
        let malicious = r#"{"error":{"code":"immutable_scope_conflict","message":"token=provider-secret path=/private/artifacts nested={\"authorization\":\"Bearer stolen\"}"}}"#;
        let forbidden = r#"{"error":{"code":"forbidden","message":"https://api.example?key=query-secret x-api-key: header-secret"}}"#;
        let (endpoint, _) = mock_server(vec![
            ("409 Conflict", "application/json", malicious),
            ("403 Forbidden", "application/json", forbidden),
        ]);
        let client = client(&endpoint, secret);
        let conflict = client.call_tool("gongbu_create_execution", create_arguments());
        let denied = client.call_tool("gongbu_get_execution", json!({"execution_id":"exec-other"}));
        let rendered = format!("{:?}{:?}", conflict.content, denied.content);
        assert!(conflict.is_error && denied.is_error);
        assert!(rendered.contains("immutable_scope_conflict") && rendered.contains("forbidden"));
        for canary in [
            secret,
            "provider-secret",
            "/private/artifacts",
            "stolen",
            "query-secret",
            "header-secret",
        ] {
            assert!(!rendered.contains(canary));
        }
    }

    #[test]
    fn operator_owned_overrides_and_unknown_fields_are_rejected_without_http() {
        let (endpoint, requests) = mock_server(vec![]);
        let client = client(&endpoint, "secret");
        for field in [
            "account_id",
            "endpoint",
            "credentials",
            "headers",
            "pricing",
            "artifact_root",
            "deadline_ms",
            "retry",
        ] {
            let mut arguments = create_arguments();
            arguments
                .as_object_mut()
                .unwrap()
                .insert(field.into(), json!("override"));
            assert!(
                client
                    .call_tool("gongbu_create_execution", arguments)
                    .is_error
            );
        }
        for field in ["account_id", "agent_id", "amount_minor", "execution_scope"] {
            let mut arguments = preview_arguments();
            arguments
                .as_object_mut()
                .unwrap()
                .insert(field.into(), json!("override"));
            assert!(
                client
                    .call_tool("gongbu_preview_authorization_scope", arguments)
                    .is_error
            );
        }
        assert!(requests.lock().unwrap().is_empty());
    }

    #[test]
    fn artifact_content_is_portable_and_has_no_storage_path() {
        let body = "png-bytes";
        let (endpoint, _) = mock_server(vec![("200 OK", "image/png", body)]);
        let result = client(&endpoint, "secret")
            .call_tool("gongbu_get_artifact", json!({"artifact_id":"artifact-1"}));
        let rendered = format!("{:?}", result.content);
        assert!(!result.is_error);
        assert!(rendered.contains(&BASE64.encode(body)));
        assert!(!rendered.contains("storage_key") && !rendered.contains("path"));
    }

    #[test]
    fn list_artifacts_removes_storage_and_redacts_sensitive_metadata() {
        let body = r#"{"schema_version":1,"execution_id":"exec-1","artifacts":[{"artifact_id":"a-1","execution_id":"exec-1","kind":"image","media_type":"image/png","size_bytes":2,"sha256":"sha256:x","metadata":{"storage_key":"private/file","nested":{"token":"canary","width":1}},"metadata_schema_version":1,"created_at":"now"}]}"#;
        let (endpoint, _) = mock_server(vec![("200 OK", "application/json", body)]);
        let result = client(&endpoint, "secret")
            .call_tool("gongbu_list_artifacts", json!({"execution_id":"exec-1"}));
        let rendered = format!("{:?}", result.content);
        assert!(!rendered.contains("private/file") && !rendered.contains("canary"));
        assert!(rendered.contains("REDACTED") && rendered.contains("width"));
    }

    #[test]
    fn opt_in_real_gongbu_execution_read() {
        if env::var("GONGBU_MCP_INTEGRATION").as_deref() != Ok("1") {
            return;
        }
        let config = Config::from_env().unwrap();
        let execution_id =
            env::var("GONGBU_MCP_INTEGRATION_EXECUTION_ID").expect("set execution ID");
        let result = GongbuClient::new(config)
            .unwrap()
            .call_tool("gongbu_get_execution", json!({"execution_id":execution_id}));
        assert!(!result.is_error, "{result:?}");
    }
}
