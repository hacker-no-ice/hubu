//! Unified MCP server for the separate Hubu and Gongbu services.
//!
//! This crate deliberately has no dependency on either backend's domain or
//! server crate. Backend clients hold independent endpoints, credentials, HTTP
//! clients, and failure boundaries. Both approved domain catalogs route through
//! public, versioned adapter contracts without importing backend implementation
//! crates.

mod gongbu;

use std::{
    env, fmt, fs,
    io::{self, BufRead, Write},
    sync::{Arc, Mutex},
    time::Duration,
};

use chrono::{SecondsFormat, Utc};
use hubu_mcp::{
    route_tool_call_v1, tool_result_v1, HubuHttpRequestV1, HubuRequestCapabilityV1,
    HUBU_ROUTING_CONTRACT_VERSION,
};
use reqwest::{
    blocking::Client,
    header::{self, HeaderMap, HeaderValue},
    Url,
};
use serde::Deserialize;
use serde_json::{json, Value};
use thiserror::Error;

mod capability;
mod diagnostics;
mod probe;

use capability::{capabilities_value, BackendState, CapabilitySnapshot};
use diagnostics::{backend_error_response, tool_availability, ToolRejection};

pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
pub const UNIFIED_CONTRACT_VERSION: &str = "hubu-gongbu-mcp-v1";
pub const EXECUTOR_CONTRACT_VERSION: &str = "hubu-spend-executor-v4.2";
pub const ROUTING_REVISION: u32 = 1;

const HUBU_ENDPOINT_ENV: &str = "HUBU_UNIFIED_HUBU_ENDPOINT";
const HUBU_TOKEN_ENV: &str = "HUBU_UNIFIED_HUBU_BEARER_TOKEN";
const GONGBU_ENDPOINT_ENV: &str = "HUBU_UNIFIED_GONGBU_ENDPOINT";
const GONGBU_TOKEN_ENV: &str = "HUBU_UNIFIED_GONGBU_BEARER_TOKEN";
const TRUST_CLIENT_APPROVAL_ENV: &str = "HUBU_MCP_TRUST_CLIENT_APPROVAL";
const RECONCILIATION_TOKEN_ENV: &str = "HUBU_RECONCILIATION_TOKEN";
const RECONCILIATION_TOKEN_FILE_ENV: &str = "HUBU_RECONCILIATION_TOKEN_FILE";
const DEFAULT_RECONCILIATION_TOKEN_FILE: &str = "hubu.reconciliation-token";
const RECONCILIATION_CAPABILITY_HEADER: &str = "X-Hubu-Reconciliation-Capability";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

const DOMAIN_TOOLS: &[(&str, BackendOwner)] = &[
    ("gongbu_create_execution", BackendOwner::Gongbu),
    ("gongbu_get_artifact", BackendOwner::Gongbu),
    ("gongbu_get_execution", BackendOwner::Gongbu),
    ("gongbu_list_artifacts", BackendOwner::Gongbu),
    ("hubu_add_policy", BackendOwner::Hubu),
    ("hubu_apply_policy", BackendOwner::Hubu),
    ("hubu_authorize_spend", BackendOwner::Hubu),
    ("hubu_client_approval_profile", BackendOwner::Hubu),
    ("hubu_create_budget", BackendOwner::Hubu),
    ("hubu_create_recurring_budget", BackendOwner::Hubu),
    ("hubu_export_policy", BackendOwner::Hubu),
    ("hubu_get_executor_claim", BackendOwner::Hubu),
    ("hubu_health", BackendOwner::Hubu),
    ("hubu_list_agents", BackendOwner::Hubu),
    ("hubu_list_budgets", BackendOwner::Hubu),
    (
        "hubu_list_claims_requiring_reconciliation",
        BackendOwner::Hubu,
    ),
    ("hubu_list_ledger", BackendOwner::Hubu),
    ("hubu_list_users", BackendOwner::Hubu),
    ("hubu_policy_diff", BackendOwner::Hubu),
    ("hubu_policy_history", BackendOwner::Hubu),
    ("hubu_reconcile_vendor_billed_claim", BackendOwner::Hubu),
    (
        "hubu_reconcile_vendor_did_not_bill_claim",
        BackendOwner::Hubu,
    ),
    ("hubu_register_agent", BackendOwner::Hubu),
    ("hubu_register_human", BackendOwner::Hubu),
    ("hubu_registration_guidance", BackendOwner::Hubu),
    ("hubu_replace_budget", BackendOwner::Hubu),
    ("hubu_revoke_budget", BackendOwner::Hubu),
    ("hubu_revoke_spending_target", BackendOwner::Hubu),
    ("hubu_set_spending_target", BackendOwner::Hubu),
    ("hubu_show_policy", BackendOwner::Hubu),
    ("hubu_show_spending_targets", BackendOwner::Hubu),
    ("hubu_submit_spend", BackendOwner::Hubu),
];

fn is_approved_hubu_tool(name: &str) -> bool {
    DOMAIN_TOOLS
        .iter()
        .any(|(candidate, owner)| *owner == BackendOwner::Hubu && *candidate == name)
}

fn is_approved_hubu_http_route(method: &str, path: &str) -> bool {
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    matches!(
        (method, path),
        ("GET", "/health")
            | ("GET", "/registration/guidance")
            | ("GET", "/users")
            | ("POST", "/init")
            | ("POST", "/agents/register")
            | ("POST", "/policies")
            | ("GET", "/policies/show")
            | ("GET", "/policies/export")
            | ("GET", "/policies/history")
            | ("GET", "/policies/diff")
            | ("POST", "/budgets")
            | ("POST", "/budgets/series")
            | ("POST", "/budgets/revoke")
            | ("POST", "/budgets/replace")
            | ("POST", "/user/spending-target")
            | ("POST", "/user/spending-target/revoke")
            | ("GET", "/user/spending-target")
            | ("POST", "/spend")
            | ("POST", "/spend/authorize")
            | ("GET", "/agents")
            | ("GET", "/budgets")
            | ("GET", "/ledger")
            | ("GET", "/spend/executor/claim")
            | ("GET", "/spend/executor/reconciliation")
            | ("POST", "/spend/executor/settle")
            | ("POST", "/spend/executor/release")
    )
}

fn unified_approval_profile() -> Value {
    let mut profile = hubu_mcp::approval_profile();
    for pointer in [
        "/client_policy/auto_approve_tools",
        "/client_policy/prompt_before_call_tools",
        "/client_policy/hubu_policy_conditional_tools",
        "/tools/0/names",
        "/tools/1/names",
        "/tools/2/names",
    ] {
        profile
            .pointer_mut(pointer)
            .and_then(Value::as_array_mut)
            .expect("standalone approval profile contract contains tool names")
            .retain(|name| name.as_str().is_some_and(is_approved_hubu_tool));
    }
    profile["response_contract"]["agent_action"] = json!(
        "Stop the spend workflow and surface approval_reason plus the structured response to the human."
    );
    profile
}

pub fn product_version() -> &'static str {
    option_env!("HUBU_PRODUCT_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

pub fn source_commit() -> &'static str {
    option_env!("HUBU_SOURCE_COMMIT").unwrap_or("unknown")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendOwner {
    Hubu,
    Gongbu,
}

impl BackendOwner {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Hubu => "hubu",
            Self::Gongbu => "gongbu",
        }
    }
}

#[derive(Clone)]
struct Secret(String);

impl Secret {
    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted>")
    }
}

#[derive(Clone, Debug)]
pub struct BackendConfig {
    owner: BackendOwner,
    endpoint: Url,
    bearer_token: Secret,
}

impl BackendConfig {
    pub fn new(
        owner: BackendOwner,
        endpoint: impl AsRef<str>,
        bearer_token: impl Into<String>,
    ) -> Result<Self, ConfigError> {
        let endpoint = validate_endpoint(owner, endpoint.as_ref())?;
        let bearer_token = bearer_token.into();
        if bearer_token.trim().is_empty() {
            return Err(ConfigError::MissingCredential(owner));
        }
        Ok(Self {
            owner,
            endpoint,
            bearer_token: Secret(bearer_token),
        })
    }

    pub fn owner(&self) -> BackendOwner {
        self.owner
    }

    pub fn endpoint(&self) -> &Url {
        &self.endpoint
    }
}

#[derive(Clone, Debug)]
pub struct HubuRoutingConfig {
    trusted_client_approval: bool,
    reconciliation_capability: Option<Secret>,
    reconciliation_capability_file: String,
}

impl HubuRoutingConfig {
    pub fn new(trusted_client_approval: bool, reconciliation_capability: Option<String>) -> Self {
        Self {
            trusted_client_approval,
            reconciliation_capability: reconciliation_capability.map(Secret),
            reconciliation_capability_file: DEFAULT_RECONCILIATION_TOKEN_FILE.to_string(),
        }
    }

    fn reconciliation_capability(&self) -> Result<Secret, HubuForwardError> {
        if let Some(capability) = &self.reconciliation_capability {
            if capability.expose().trim().is_empty() {
                return Err(HubuForwardError::InvalidReconciliationCapability);
            }
            return Ok(capability.clone());
        }
        match fs::read_to_string(&self.reconciliation_capability_file) {
            Ok(contents) if contents.trim().is_empty() => {
                Err(HubuForwardError::InvalidReconciliationCapability)
            }
            Ok(contents) => Ok(Secret(contents.trim().to_string())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Err(HubuForwardError::MissingReconciliationCapability)
            }
            Err(_) => Err(HubuForwardError::InvalidReconciliationCapability),
        }
    }
}

impl Default for HubuRoutingConfig {
    fn default() -> Self {
        Self::new(false, None)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub hubu: Option<BackendConfig>,
    pub gongbu: Option<BackendConfig>,
    pub hubu_routing: HubuRoutingConfig,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| env::var(name).ok())
    }

    fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let mut hubu_routing = HubuRoutingConfig::new(
            lookup(TRUST_CLIENT_APPROVAL_ENV).is_some_and(|value| env_flag_value(&value)),
            lookup(RECONCILIATION_TOKEN_ENV),
        );
        hubu_routing.reconciliation_capability_file = lookup(RECONCILIATION_TOKEN_FILE_ENV)
            .unwrap_or_else(|| DEFAULT_RECONCILIATION_TOKEN_FILE.to_string());
        Ok(Self {
            hubu: backend_from_values(
                BackendOwner::Hubu,
                lookup(HUBU_ENDPOINT_ENV),
                lookup(HUBU_TOKEN_ENV),
            )?,
            gongbu: backend_from_values(
                BackendOwner::Gongbu,
                lookup(GONGBU_ENDPOINT_ENV),
                lookup(GONGBU_TOKEN_ENV),
            )?,
            hubu_routing,
        })
    }
}

fn env_flag_value(value: &str) -> bool {
    matches!(value, "1" | "true" | "TRUE" | "yes" | "YES")
}

fn backend_from_values(
    owner: BackendOwner,
    endpoint: Option<String>,
    token: Option<String>,
) -> Result<Option<BackendConfig>, ConfigError> {
    let endpoint = endpoint.filter(|value| !value.trim().is_empty());
    let token = token.filter(|value| !value.trim().is_empty());
    match (endpoint, token) {
        (None, None) => Ok(None),
        // A partial pair is unconfigured rather than a process-wide startup
        // failure. This preserves the unrelated backend's failure domain.
        (None, Some(_)) | (Some(_), None) => Ok(None),
        (Some(endpoint), Some(token)) => BackendConfig::new(owner, endpoint, token).map(Some),
    }
}

fn validate_endpoint(owner: BackendOwner, endpoint: &str) -> Result<Url, ConfigError> {
    let mut url = Url::parse(endpoint).map_err(|_| ConfigError::InvalidEndpoint(owner))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.cannot_be_a_base()
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ConfigError::InvalidEndpoint(owner));
    }
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Ok(url)
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("{0} backend endpoint is required when its credential is configured")]
    MissingEndpoint(BackendOwner),
    #[error("{0} backend credential is required when its endpoint is configured")]
    MissingCredential(BackendOwner),
    #[error(
        "{0} backend endpoint must be an HTTP(S) base URL without credentials, query, or fragment"
    )]
    InvalidEndpoint(BackendOwner),
}

impl fmt::Display for BackendOwner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone)]
pub struct BackendClient {
    owner: BackendOwner,
    endpoint: Url,
    bearer_token: Secret,
    http: Client,
}

impl fmt::Debug for BackendClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BackendClient")
            .field("owner", &self.owner)
            .field("endpoint", &self.endpoint)
            .field("executor_contract", &EXECUTOR_CONTRACT_VERSION)
            .field("hubu_routing_contract", &HUBU_ROUTING_CONTRACT_VERSION)
            .finish_non_exhaustive()
    }
}

impl BackendClient {
    fn new(config: BackendConfig) -> Result<Self, ConfigError> {
        let mut headers = HeaderMap::new();
        let authorization =
            HeaderValue::from_str(&format!("Bearer {}", config.bearer_token.expose()))
                .map_err(|_| ConfigError::MissingCredential(config.owner))?;
        headers.insert(header::AUTHORIZATION, authorization);
        let http = Client::builder()
            .default_headers(headers)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| ConfigError::InvalidEndpoint(config.owner))?;
        Ok(Self {
            owner: config.owner,
            endpoint: config.endpoint,
            bearer_token: config.bearer_token,
            http,
        })
    }

    pub fn owner(&self) -> BackendOwner {
        self.owner
    }

    pub fn endpoint(&self) -> &Url {
        &self.endpoint
    }

    pub fn contract_version(&self) -> &'static str {
        EXECUTOR_CONTRACT_VERSION
    }

    pub fn http_client(&self) -> &Client {
        &self.http
    }

    fn execute_hubu(
        &self,
        request: HubuHttpRequestV1,
        routing: &HubuRoutingConfig,
    ) -> anyhow::Result<Value> {
        debug_assert_eq!(self.owner, BackendOwner::Hubu);
        if !is_approved_hubu_http_route(request.method, &request.path) {
            return Err(HubuForwardError::InvalidRoute.into());
        }
        let is_read = request.method == "GET";
        let url = self
            .endpoint
            .join(request.path.trim_start_matches('/'))
            .map_err(|_| HubuForwardError::InvalidRoute)?;
        let mut builder = match request.method {
            "GET" => self.http.get(url),
            "POST" => self.http.post(url),
            _ => return Err(HubuForwardError::InvalidRoute.into()),
        };
        if let Some(body) = request.body {
            builder = builder.json(&body);
        }
        let mut used_reconciliation_capability = None;
        match request.capability {
            HubuRequestCapabilityV1::None => {}
            HubuRequestCapabilityV1::Reconciliation => {
                let capability = routing.reconciliation_capability()?;
                builder = builder.header(RECONCILIATION_CAPABILITY_HEADER, capability.expose());
                used_reconciliation_capability = Some(capability);
            }
            HubuRequestCapabilityV1::Approval => {
                return Err(HubuForwardError::UnsupportedCapability.into());
            }
        }

        let response = builder.send().map_err(|error| {
            if error.is_connect() || is_read {
                HubuForwardError::Unavailable
            } else {
                HubuForwardError::AmbiguousTransport
            }
        })?;
        let status = response.status();
        let body = response.json::<Value>().map_err(|error| {
            if is_read && (error.is_timeout() || error.is_body()) {
                HubuForwardError::Unavailable
            } else if is_read {
                HubuForwardError::InvalidResponse
            } else {
                HubuForwardError::AmbiguousTransport
            }
        })?;
        if !status.is_success() {
            let message = body
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("request failed");
            let message = redact_backend_message(
                message,
                &self.bearer_token,
                routing.reconciliation_capability.as_ref(),
                used_reconciliation_capability.as_ref(),
            );
            return Err(HubuForwardError::Application {
                status: status.as_u16(),
                message,
            }
            .into());
        }
        Ok(body)
    }
}

fn redact_backend_message(
    message: &str,
    bearer_token: &Secret,
    configured_reconciliation: Option<&Secret>,
    used_reconciliation: Option<&Secret>,
) -> String {
    let mut redacted = message.replace(bearer_token.expose(), "<redacted>");
    for secret in [configured_reconciliation, used_reconciliation]
        .into_iter()
        .flatten()
    {
        if !secret.expose().is_empty() {
            redacted = redacted.replace(secret.expose(), "<redacted>");
        }
    }
    redacted
}

#[derive(Debug, Error)]
enum HubuForwardError {
    #[error("Hubu backend is unavailable")]
    Unavailable,
    #[error("Hubu backend request failed after dispatch; mutation outcome may be ambiguous")]
    AmbiguousTransport,
    #[error("Hubu backend returned an invalid JSON response")]
    InvalidResponse,
    #[error("Hubu route is invalid")]
    InvalidRoute,
    #[error(
        "human reconciliation requires HUBU_RECONCILIATION_TOKEN or HUBU_RECONCILIATION_TOKEN_FILE"
    )]
    MissingReconciliationCapability,
    #[error("Hubu reconciliation credential is invalid")]
    InvalidReconciliationCapability,
    #[error("Hubu approval capability is not supported by the HUB-88 routing contract")]
    UnsupportedCapability,
    #[error("Hubu server returned HTTP {status}: {message}")]
    Application { status: u16, message: String },
}

/// Versioned boundary implemented by each independently configured backend.
///
/// Routing issues can extend this interface with contract requests without
/// importing either backend's domain or server crate.
pub trait BackendAdapter {
    fn owner(&self) -> BackendOwner;
    fn endpoint(&self) -> &Url;
    fn contract_version(&self) -> &'static str;
}

impl BackendAdapter for BackendClient {
    fn owner(&self) -> BackendOwner {
        self.owner()
    }

    fn endpoint(&self) -> &Url {
        self.endpoint()
    }

    fn contract_version(&self) -> &'static str {
        self.contract_version()
    }
}

#[derive(Clone, Debug)]
pub struct BackendClients {
    pub hubu: Option<BackendClient>,
    pub gongbu: Option<BackendClient>,
}

impl BackendClients {
    pub fn new(config: Config) -> Result<Self, ConfigError> {
        Ok(Self {
            hubu: config.hubu.map(BackendClient::new).transpose()?,
            gongbu: config.gongbu.map(BackendClient::new).transpose()?,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Server {
    backends: BackendClients,
    snapshot: Arc<Mutex<CapabilitySnapshot>>,
    hubu_routing: HubuRoutingConfig,
}

impl Server {
    pub fn new(config: Config) -> Result<Self, ConfigError> {
        let hubu_routing = config.hubu_routing.clone();
        let backends = BackendClients::new(config)?;
        let snapshot = backends.probe();
        Ok(Self {
            backends,
            snapshot: Arc::new(Mutex::new(snapshot)),
            hubu_routing,
        })
    }

    pub fn run(self, input: impl BufRead, mut output: impl Write) -> io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Some(response) = self.handle_line(&line) {
                serde_json::to_writer(&mut output, &response)?;
                output.write_all(b"\n")?;
                output.flush()?;
            }
        }
        Ok(())
    }

    fn handle_line(&self, line: &str) -> Option<Value> {
        let request: Request = match serde_json::from_str(line) {
            Ok(request) => request,
            Err(_) => return Some(error_response(Value::Null, -32700, "Parse error")),
        };
        let id = request.id?;
        if request.jsonrpc.as_deref() != Some("2.0") || request.method.is_empty() {
            return Some(error_response(id, -32600, "Invalid Request"));
        }
        let response = match request.method.as_str() {
            "initialize" => success_response(id, self.initialize_result()),
            "ping" => success_response(id, json!({})),
            "tools/list" => success_response(id, json!({ "tools": self.list_tools() })),
            "tools/call" => self.call_tool(id, request.params),
            _ => error_response(id, -32601, "Method not found"),
        };
        Some(response)
    }

    fn initialize_result(&self) -> Value {
        self.refresh_capabilities();
        let mut capability = self.capabilities();
        capability
            .as_object_mut()
            .expect("capability snapshot is an object")
            .remove("generated_at");
        json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "serverInfo": {
                "name": "hubu-unified-mcp",
                "version": product_version()
            },
            "capabilities": {
                "tools": { "listChanged": true },
                "experimental": {
                    "hubu.dev/unified-mcp": capability
                }
            }
        })
    }

    fn call_tool(&self, id: Value, params: Value) -> Value {
        let call: ToolCall = match serde_json::from_value::<ToolCall>(params) {
            Ok(call) => call,
            Err(_) => return error_response(id, -32602, "Invalid params"),
        };
        if !call.arguments.is_object() {
            return error_response(id, -32602, "Invalid params");
        }
        if call.name == "hubu_unified_capabilities" {
            self.refresh_capabilities();
            if call
                .arguments
                .as_object()
                .is_some_and(|arguments| arguments.is_empty())
            {
                let capability = self.capabilities();
                return success_response(
                    id,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string_pretty(&capability)
                                .expect("capability snapshot serializes")
                        }],
                        "structuredContent": capability
                    }),
                );
            }
            return error_response(id, -32602, "Invalid params");
        }
        let Some(owner) = DOMAIN_TOOLS
            .iter()
            .find_map(|(name, owner)| (*name == call.name).then_some(*owner))
        else {
            return error_response(id, -32602, "Invalid params");
        };
        if owner == BackendOwner::Gongbu {
            self.refresh_capabilities();
            let snapshot = self.snapshot();
            if let Err(rejection) = tool_availability(&call.name, owner, &snapshot) {
                return backend_error_response(id, &call.name, owner, rejection);
            }
            let client = self
                .backends
                .gongbu
                .as_ref()
                .expect("available Gongbu route has a configured client");
            return success_response(id, gongbu::call_tool(client, &call.name, call.arguments));
        }
        self.refresh_hubu_capability();
        self.call_approved_hubu_tool(id, call)
    }

    fn call_approved_hubu_tool(&self, id: Value, call: ToolCall) -> Value {
        let snapshot = self.snapshot();
        if let Err(rejection) = tool_availability(&call.name, BackendOwner::Hubu, &snapshot) {
            return backend_error_response(id, &call.name, BackendOwner::Hubu, rejection);
        }
        let Some(hubu) = self.backends.hubu.as_ref() else {
            return backend_error_response(
                id,
                &call.name,
                BackendOwner::Hubu,
                ToolRejection::Unconfigured,
            );
        };
        let name = call.name;
        if name == "hubu_client_approval_profile" {
            return success_response(id, tool_result_v1(unified_approval_profile()));
        }
        let params = json!({
            "name": name,
            "arguments": call.arguments,
            "_meta": call.meta
        });
        match route_tool_call_v1(
            params,
            self.hubu_routing.trusted_client_approval,
            |request| hubu.execute_hubu(request, &self.hubu_routing),
        ) {
            Ok(result) => success_response(id, result),
            Err(error)
                if matches!(
                    error.downcast_ref::<HubuForwardError>(),
                    Some(HubuForwardError::Unavailable)
                ) =>
            {
                self.mark_hubu_unavailable();
                backend_error_response(id, &name, BackendOwner::Hubu, ToolRejection::Unavailable)
            }
            Err(error) => error_response(id, -32000, &error.to_string()),
        }
    }

    #[cfg(test)]
    fn call_tool_from_snapshot(&self, id: Value, params: Value) -> Value {
        let call: ToolCall = match serde_json::from_value::<ToolCall>(params) {
            Ok(call) if call.arguments.is_object() => call,
            _ => return error_response(id, -32602, "Invalid params"),
        };
        if call.name == "hubu_unified_capabilities" {
            if call
                .arguments
                .as_object()
                .is_some_and(|value| value.is_empty())
            {
                let capability = self.capabilities();
                return success_response(
                    id,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string_pretty(&capability)
                                .expect("capability snapshot serializes")
                        }],
                        "structuredContent": capability
                    }),
                );
            }
            return error_response(id, -32602, "Invalid params");
        }
        let Some(owner) = DOMAIN_TOOLS
            .iter()
            .find_map(|(name, owner)| (*name == call.name).then_some(*owner))
        else {
            return error_response(id, -32602, "Invalid params");
        };
        let snapshot = self.snapshot();
        if let Err(rejection) = tool_availability(&call.name, owner, &snapshot) {
            return backend_error_response(id, &call.name, owner, rejection);
        }
        if owner == BackendOwner::Gongbu {
            let client = self
                .backends
                .gongbu
                .as_ref()
                .expect("available Gongbu route has a configured client");
            return success_response(id, gongbu::call_tool(client, &call.name, call.arguments));
        }
        self.call_approved_hubu_tool(id, call)
    }

    fn list_tools(&self) -> Vec<Value> {
        self.refresh_capabilities();
        self.list_tools_for_snapshot()
    }

    fn list_tools_for_snapshot(&self) -> Vec<Value> {
        let snapshot = self.snapshot();
        let mut tools = vec![capability_tool()];
        if tool_availability("hubu_health", BackendOwner::Hubu, &snapshot).is_ok() {
            tools.extend(
                hubu_mcp::tool_definitions()
                    .into_iter()
                    .filter(|tool| tool["name"].as_str().is_some_and(is_approved_hubu_tool)),
            );
        }
        tools.extend(gongbu::tool_definitions().into_iter().filter(|tool| {
            let name = tool["name"]
                .as_str()
                .expect("Gongbu tool definition has a name");
            tool_availability(name, BackendOwner::Gongbu, &snapshot).is_ok()
        }));
        tools
    }

    fn capabilities(&self) -> Value {
        capabilities_value(&self.snapshot())
    }

    fn snapshot(&self) -> CapabilitySnapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn refresh_capabilities(&self) {
        let refreshed = self.backends.probe();
        *self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = refreshed;
    }

    fn refresh_hubu_capability(&self) {
        let refreshed = self.backends.probe_hubu();
        let mut snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        snapshot.generated_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        snapshot.hubu = refreshed;
    }

    fn mark_hubu_unavailable(&self) {
        let mut snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        snapshot.generated_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        snapshot.hubu.state = BackendState::Unavailable;
        snapshot.hubu.reason_code = Some("health_unavailable");
    }
}

fn capability_tool() -> Value {
    json!({
        "name": "hubu_unified_capabilities",
        "description": "Read the unified MCP contract and current backend capability snapshot.",
        "inputSchema": {
            "type": "object",
            "properties": {},
            "required": [],
            "additionalProperties": false
        },
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false
        }
    })
}

#[derive(Deserialize)]
struct Request {
    jsonrpc: Option<String>,
    id: Option<Value>,
    #[serde(default)]
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolCall {
    name: String,
    #[serde(default = "empty_object")]
    arguments: Value,
    #[serde(default, rename = "_meta")]
    meta: Option<Value>,
}

fn empty_object() -> Value {
    json!({})
}

fn success_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i32, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

pub fn run_stdio_from_env(input: impl BufRead, output: impl Write) -> Result<(), StartupError> {
    let config = Config::from_env()?;
    Server::new(config)?.run(input, output)?;
    Ok(())
}

#[derive(Debug, Error)]
pub enum StartupError {
    #[error("configuration error: {0}")]
    Config(#[from] ConfigError),
    #[error("stdio transport failed: {0}")]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{BackendReport, ContractVersions};
    use std::{
        io::{Cursor, Read},
        net::TcpListener,
        sync::mpsc::{self, Receiver},
        thread,
        time::Duration,
    };

    fn lookup<'a>(values: &'a [(&'a str, &'a str)]) -> impl FnMut(&str) -> Option<String> + 'a {
        move |name| {
            values
                .iter()
                .find_map(|(key, value)| (*key == name).then(|| (*value).to_owned()))
        }
    }

    fn server_with_backends(
        hubu_endpoint: &str,
        gongbu_endpoint: Option<&str>,
        trusted_client_approval: bool,
        reconciliation_capability: Option<&str>,
    ) -> Server {
        let config = Config {
            hubu: Some(
                BackendConfig::new(BackendOwner::Hubu, hubu_endpoint, "hubu-token-canary").unwrap(),
            ),
            gongbu: gongbu_endpoint.map(|endpoint| {
                BackendConfig::new(BackendOwner::Gongbu, endpoint, "gongbu-token-canary").unwrap()
            }),
            hubu_routing: HubuRoutingConfig::new(
                trusted_client_approval,
                reconciliation_capability.map(str::to_string),
            ),
        };
        let hubu_routing = config.hubu_routing.clone();
        let gongbu_state = if config.gongbu.is_some() {
            BackendState::Available
        } else {
            BackendState::Unconfigured
        };
        Server {
            backends: BackendClients::new(config).unwrap(),
            snapshot: Arc::new(Mutex::new(CapabilitySnapshot {
                generated_at: "2026-08-18T00:00:00.000Z".into(),
                hubu: test_backend_report(BackendState::Available, false),
                gongbu: test_backend_report(gongbu_state, true),
            })),
            hubu_routing,
        }
    }

    fn test_backend_report(state: BackendState, gongbu: bool) -> BackendReport {
        BackendReport {
            state,
            product_version: Some(product_version().into()),
            source_commit: Some("a".repeat(40)),
            api_schema_version: gongbu.then_some(2),
            mcp_schema_version: gongbu.then_some(2),
            contract_versions: ContractVersions {
                executor: Some(EXECUTOR_CONTRACT_VERSION.into()),
            },
            reason_code: (state != BackendState::Available).then_some("configuration_missing"),
        }
    }

    fn tool_call(server: &Server, name: &str, arguments: Value, meta: Option<Value>) -> Value {
        server.call_tool_from_snapshot(
            json!(7),
            json!({"name": name, "arguments": arguments, "_meta": meta}),
        )
    }

    fn one_shot_http_server(
        status: u16,
        body: &'static str,
    ) -> (String, Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let count = stream.read(&mut buffer).unwrap();
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n")
                {
                    let header_end = header_end + 4;
                    let headers = String::from_utf8_lossy(&bytes[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(str::trim)
                                .and_then(|value| value.parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= header_end + content_length {
                        break;
                    }
                }
            }
            sender.send(String::from_utf8(bytes).unwrap()).unwrap();
            write!(
                stream,
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        (endpoint, receiver, handle)
    }

    fn disconnect_after_request_server() -> (String, Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut buffer = [0_u8; 2048];
            let count = stream.read(&mut buffer).unwrap();
            sender
                .send(String::from_utf8(buffer[..count].to_vec()).unwrap())
                .unwrap();
        });
        (endpoint, receiver, handle)
    }

    #[test]
    fn validates_independent_backend_configuration() {
        let config = Config::from_lookup(lookup(&[
            (HUBU_ENDPOINT_ENV, "https://hubu.example.test/api"),
            (HUBU_TOKEN_ENV, "hubu-secret"),
            (GONGBU_ENDPOINT_ENV, "http://127.0.0.1:8788"),
            (GONGBU_TOKEN_ENV, "gongbu-secret"),
        ]))
        .unwrap();

        assert_eq!(config.hubu.as_ref().unwrap().owner(), BackendOwner::Hubu);
        assert_eq!(
            config.hubu.as_ref().unwrap().endpoint().as_str(),
            "https://hubu.example.test/api/"
        );
        assert_eq!(
            config.gongbu.as_ref().unwrap().endpoint().as_str(),
            "http://127.0.0.1:8788/"
        );
    }

    #[test]
    fn incomplete_pair_is_unconfigured_without_blocking_the_other_backend() {
        let config = Config::from_lookup(lookup(&[
            (HUBU_ENDPOINT_ENV, "http://hubu.test"),
            (HUBU_TOKEN_ENV, "hubu-secret"),
            (GONGBU_ENDPOINT_ENV, "http://gongbu.test"),
        ]))
        .unwrap();

        assert!(config.hubu.is_some());
        assert!(config.gongbu.is_none());
        let capability = Server::new(config).unwrap().capabilities();
        assert_eq!(capability["backends"]["hubu"]["state"], "unavailable");
        assert_eq!(capability["backends"]["gongbu"]["state"], "unconfigured");
    }

    #[test]
    fn rejects_credential_bearing_endpoint_configuration() {
        assert_eq!(
            Config::from_lookup(lookup(&[
                (GONGBU_ENDPOINT_ENV, "https://secret@gongbu.test"),
                (GONGBU_TOKEN_ENV, "never-print-me"),
            ]))
            .unwrap_err(),
            ConfigError::InvalidEndpoint(BackendOwner::Gongbu)
        );
    }

    #[test]
    fn configuration_diagnostics_redact_credentials() {
        let secret = "credential-that-must-not-appear";
        let config = BackendConfig::new(BackendOwner::Hubu, "https://hubu.test", secret).unwrap();
        let debug = format!("{config:?}");
        assert!(!debug.contains(secret));

        let error = BackendConfig::new(BackendOwner::Hubu, "https://url-secret@hubu.test", secret)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("url-secret"));
        assert!(!error.contains(secret));

        let routing = HubuRoutingConfig::new(true, Some(secret.to_string()));
        assert!(!format!("{routing:?}").contains(secret));
    }

    #[test]
    fn initialize_and_list_tools_expose_only_the_shell() {
        let server = Server::new(Config::default()).unwrap();
        let input = Cursor::new(
            concat!(
                "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n",
                "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n"
            )
            .as_bytes(),
        );
        let mut output = Vec::new();
        server.run(input, &mut output).unwrap();
        let responses = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(
            responses[0]["result"]["protocolVersion"],
            MCP_PROTOCOL_VERSION
        );
        assert_eq!(
            responses[0]["result"]["serverInfo"]["name"],
            "hubu-unified-mcp"
        );
        assert_eq!(
            responses[0]["result"]["capabilities"]["tools"]["listChanged"],
            true
        );
        let tools = responses[1]["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "hubu_unified_capabilities");
    }

    #[test]
    fn capability_call_accepts_trusted_metadata() {
        let server = Server::new(Config::default()).unwrap();
        let input = Cursor::new(
            concat!(
                "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",",
                "\"params\":{\"name\":\"hubu_unified_capabilities\",\"arguments\":{},",
                "\"_meta\":{\"hubu.dev/platform-invocation\":{\"operation_key\":\"opaque\"}}}}\n"
            )
            .as_bytes(),
        );
        let mut output = Vec::new();
        server.run(input, &mut output).unwrap();
        let response: Value = serde_json::from_slice(&output).unwrap();

        assert!(response.get("error").is_none());
        assert_eq!(
            response["result"]["structuredContent"]["contract_version"],
            UNIFIED_CONTRACT_VERSION
        );
    }

    #[test]
    fn capability_placeholder_is_contract_shaped_and_secret_free() {
        let config = Config {
            hubu: Some(
                BackendConfig::new(BackendOwner::Hubu, "https://hubu.test", "hubu-secret").unwrap(),
            ),
            gongbu: Some(
                BackendConfig::new(BackendOwner::Gongbu, "https://gongbu.test", "gongbu-secret")
                    .unwrap(),
            ),
            hubu_routing: HubuRoutingConfig::default(),
        };
        let server = Server::new(config).unwrap();
        let capability = server.capabilities();
        let serialized = capability.to_string();

        assert_eq!(capability["contract_version"], UNIFIED_CONTRACT_VERSION);
        assert_eq!(capability["tools"].as_array().unwrap().len(), 33);
        assert_eq!(capability["backends"]["hubu"]["state"], "unavailable");
        assert!(!serialized.contains("hubu.test"));
        assert!(!serialized.contains("gongbu.test"));
        assert!(!serialized.contains("secret"));
    }

    #[test]
    fn initialize_extension_matches_capability_schema_without_timestamp() {
        let server = Server::new(Config::default()).unwrap();
        let mut capability = server.capabilities();
        capability.as_object_mut().unwrap().remove("generated_at");
        assert_eq!(
            server.initialize_result()["capabilities"]["experimental"]["hubu.dev/unified-mcp"],
            capability
        );
    }

    #[test]
    fn configured_catalog_matches_the_approved_standalone_hubu_contract() {
        let server = server_with_backends("http://127.0.0.1:1", None, false, None);
        let actual = server.list_tools_for_snapshot();
        assert_eq!(actual[0], capability_tool());

        let expected = hubu_mcp::tool_definitions()
            .into_iter()
            .filter(|tool| tool["name"].as_str().is_some_and(is_approved_hubu_tool))
            .collect::<Vec<_>>();
        assert_eq!(&actual[1..], expected.as_slice());
        assert_eq!(expected.len(), 28);
        assert!(!actual.iter().any(|tool| {
            matches!(
                tool["name"].as_str(),
                Some("hubu_get_spend_approval" | "hubu_resolve_spend_approval")
            )
        }));
        assert!(!actual.iter().any(|tool| tool["name"]
            .as_str()
            .is_some_and(|name| name.starts_with("gongbu_"))));
    }

    #[test]
    fn combined_catalog_exposes_both_approved_sets_under_readiness_gates() {
        let server = server_with_backends(
            "http://127.0.0.1:1",
            Some("http://127.0.0.1:2"),
            false,
            None,
        );
        let tools = server.list_tools_for_snapshot();
        assert_eq!(tools.len(), 33);
        for definition in hubu_mcp::tool_definitions()
            .into_iter()
            .filter(|tool| tool["name"].as_str().is_some_and(is_approved_hubu_tool))
            .chain(gongbu::tool_definitions())
        {
            assert!(tools.contains(&definition), "{}", definition["name"]);
        }

        {
            let mut snapshot = server
                .snapshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            snapshot.gongbu.state = BackendState::Degraded;
            snapshot.gongbu.reason_code = Some("backend_not_ready");
        }
        let degraded = server.list_tools_for_snapshot();
        assert_eq!(degraded.len(), 32);
        assert!(!degraded
            .iter()
            .any(|tool| tool["name"] == "gongbu_create_execution"));
        assert!(degraded
            .iter()
            .any(|tool| tool["name"] == "gongbu_get_execution"));

        {
            let mut snapshot = server
                .snapshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            snapshot.gongbu.state = BackendState::Available;
            snapshot.gongbu.reason_code = None;
            snapshot.hubu.state = BackendState::Unavailable;
            snapshot.hubu.reason_code = Some("health_unavailable");
        }
        let hubu_down = server.list_tools_for_snapshot();
        assert_eq!(hubu_down.len(), 4);
        assert!(!hubu_down.iter().any(|tool| tool["name"]
            .as_str()
            .is_some_and(|name| name.starts_with("hubu_") && name != "hubu_unified_capabilities")));
        assert!(!hubu_down
            .iter()
            .any(|tool| tool["name"] == "gongbu_create_execution"));
    }

    #[test]
    fn unified_approval_profile_contains_only_callable_continuations() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = server_with_backends(&endpoint, None, false, None);

        let response = tool_call(&server, "hubu_client_approval_profile", json!({}), None);
        let profile = &response["result"]["structuredContent"];
        let serialized = profile.to_string();
        assert!(!serialized.contains("hubu_get_spend_approval"));
        assert!(!serialized.contains("hubu_resolve_spend_approval"));
        assert_eq!(
            profile["response_contract"]["agent_action"],
            "Stop the spend workflow and surface approval_reason plus the structured response to the human."
        );
        assert_eq!(
            response["result"]["content"][0]["text"],
            serde_json::to_string_pretty(profile).unwrap()
        );
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
        );
    }

    #[test]
    fn approved_hubu_routes_prepare_exact_static_requests() {
        let empty = json!({});
        let spend = json!({"account_id":"account-1","amount_cents":25,"reason":"test"});
        let reconciliation = json!({
            "claim_id":"claim-1",
            "provider_reference":"provider-1",
            "evidence":"reviewed"
        });
        let cases = [
            (
                "hubu_health",
                empty.clone(),
                "GET",
                "/health",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_registration_guidance",
                empty.clone(),
                "GET",
                "/registration/guidance",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_list_users",
                empty.clone(),
                "GET",
                "/users",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_register_human",
                empty.clone(),
                "POST",
                "/init",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_register_agent",
                empty.clone(),
                "POST",
                "/agents/register",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_add_policy",
                empty.clone(),
                "POST",
                "/policies",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_apply_policy",
                json!({"policy_yaml":"version: 1"}),
                "POST",
                "/policies",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_show_policy",
                empty.clone(),
                "GET",
                "/policies/show",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_export_policy",
                empty.clone(),
                "GET",
                "/policies/export",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_policy_history",
                empty.clone(),
                "GET",
                "/policies/history",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_policy_diff",
                json!({"from_revision":1}),
                "GET",
                "/policies/diff?from_revision=1",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_create_budget",
                empty.clone(),
                "POST",
                "/budgets",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_create_recurring_budget",
                empty.clone(),
                "POST",
                "/budgets/series",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_revoke_budget",
                empty.clone(),
                "POST",
                "/budgets/revoke",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_replace_budget",
                empty.clone(),
                "POST",
                "/budgets/replace",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_set_spending_target",
                empty.clone(),
                "POST",
                "/user/spending-target",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_revoke_spending_target",
                empty.clone(),
                "POST",
                "/user/spending-target/revoke",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_show_spending_targets",
                empty.clone(),
                "GET",
                "/user/spending-target",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_submit_spend",
                spend.clone(),
                "POST",
                "/spend",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_authorize_spend",
                spend,
                "POST",
                "/spend/authorize",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_list_agents",
                empty.clone(),
                "GET",
                "/agents",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_list_budgets",
                empty.clone(),
                "GET",
                "/budgets",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_list_ledger",
                empty.clone(),
                "GET",
                "/ledger",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_get_executor_claim",
                json!({"claim_id":"claim-1"}),
                "GET",
                "/spend/executor/claim?claim_id=claim-1",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_list_claims_requiring_reconciliation",
                empty.clone(),
                "GET",
                "/spend/executor/reconciliation",
                HubuRequestCapabilityV1::None,
            ),
            (
                "hubu_reconcile_vendor_billed_claim",
                reconciliation.clone(),
                "POST",
                "/spend/executor/settle",
                HubuRequestCapabilityV1::Reconciliation,
            ),
            (
                "hubu_reconcile_vendor_did_not_bill_claim",
                reconciliation,
                "POST",
                "/spend/executor/release",
                HubuRequestCapabilityV1::Reconciliation,
            ),
        ];
        assert_eq!(cases.len(), 27);
        for (name, arguments, method, path, capability) in cases {
            let params = json!({
                "name": name,
                "arguments": arguments,
                "_meta": {
                    "hubu.dev/platform-invocation": {
                        "platform":"codex",
                        "installation_id":"installation-1",
                        "invocation_id":"invocation-1",
                        "operation_key":"operation-1",
                        "task_id":"linear:HUB-91"
                    }
                }
            });
            let mut captured = None;
            let result = route_tool_call_v1(params, true, |request| {
                captured = Some(request);
                Ok(json!({"status":"ok"}))
            })
            .unwrap();
            let request = captured.expect("routed Hubu tool should make one request");
            assert_eq!(request.method, method, "{name}");
            assert_eq!(request.path, path, "{name}");
            assert_eq!(request.capability, capability, "{name}");
            assert!(request
                .body
                .as_ref()
                .is_none_or(|body| body.get("_meta").is_none()));
            if matches!(name, "hubu_submit_spend" | "hubu_authorize_spend") {
                assert_eq!(
                    request.body.as_ref().unwrap()["operation_key"],
                    "operation-1"
                );
                assert_eq!(request.body.as_ref().unwrap()["task_id"], "linear:HUB-91");
            }
            if name == "hubu_apply_policy" {
                assert_eq!(request.body.as_ref().unwrap()["source"], "mcp");
            }
            assert_eq!(result["structuredContent"]["status"], "ok");
        }

        let mut called = false;
        let local = route_tool_call_v1(
            json!({"name":"hubu_client_approval_profile","arguments":{}}),
            true,
            |_| {
                called = true;
                Ok(json!({}))
            },
        )
        .unwrap();
        assert!(!called);
        assert_eq!(local["structuredContent"], hubu_mcp::approval_profile());
    }

    #[test]
    fn approved_query_variants_match_standalone_routing() {
        let cases = [
            (
                "hubu_show_policy",
                json!({"policy_id":"policy-1"}),
                "/policies/show?policy_id=policy-1",
            ),
            (
                "hubu_export_policy",
                json!({"agent_id":"agent-1"}),
                "/policies/export?agent_id=agent-1",
            ),
            (
                "hubu_policy_diff",
                json!({"agent_id":"agent-1","from_revision":2,"to_revision":4}),
                "/policies/diff?agent_id=agent-1&from_revision=2&to_revision=4",
            ),
            (
                "hubu_show_spending_targets",
                json!({"include_all":true}),
                "/user/spending-target?all=true",
            ),
            (
                "hubu_list_budgets",
                json!({"include_all":true}),
                "/budgets?all=true",
            ),
        ];
        for (name, arguments, expected_path) in cases {
            let mut captured = None;
            route_tool_call_v1(
                json!({"name":name,"arguments":arguments}),
                false,
                |request| {
                    captured = Some(request);
                    Ok(json!({}))
                },
            )
            .unwrap();
            assert_eq!(captured.unwrap().path, expected_path, "{name}");
        }

        let error = route_tool_call_v1(
            json!({
                "name":"hubu_show_policy",
                "arguments":{"policy_id":"policy-1","agent_id":"agent-1"}
            }),
            false,
            |_| unreachable!(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("pass only one"));
    }

    #[test]
    fn routed_success_preserves_metadata_auth_and_spend_result_shape() {
        let (endpoint, request, handle) =
            one_shot_http_server(200, r#"{"decision":"needs_approval","payment":null}"#);
        let server = server_with_backends(&endpoint, None, false, None);
        let arguments = json!({"account_id":"account-1","amount_cents":25,"reason":"review"});
        let meta = json!({"hubu.dev/platform-invocation":{
            "platform":"codex",
            "installation_id":"installation-1",
            "invocation_id":"invocation-1",
            "operation_key":"operation-1",
            "task_id":"linear:HUB-91"
        }});
        let standalone_result = route_tool_call_v1(
            json!({
                "name":"hubu_authorize_spend",
                "arguments":arguments.clone(),
                "_meta":meta.clone()
            }),
            false,
            |_| Ok(json!({"decision":"needs_approval","payment":null})),
        )
        .unwrap();
        let response = tool_call(&server, "hubu_authorize_spend", arguments, Some(meta));
        let raw = request.recv_timeout(Duration::from_secs(2)).unwrap();
        handle.join().unwrap();

        assert!(raw.starts_with("POST /spend/authorize HTTP/1.1"));
        assert!(raw.contains("authorization: Bearer hubu-token-canary"));
        assert!(!raw.contains("gongbu-token-canary"));
        let body: Value = serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["operation_key"], "operation-1");
        assert_eq!(body["task_id"], "linear:HUB-91");
        assert!(body.get("_meta").is_none());
        assert!(body.get("platform").is_none());
        assert_eq!(response["result"], standalone_result);
        assert_eq!(
            response["result"]["structuredContent"]["requires_human_approval"],
            true
        );
        assert_eq!(
            response["result"]["content"][0]["text"],
            serde_json::to_string_pretty(&response["result"]["structuredContent"]).unwrap()
        );
    }

    #[test]
    fn reconciliation_uses_distinct_hubu_capability_and_never_gongbu() {
        let (hubu_endpoint, request, handle) = one_shot_http_server(200, r#"{"status":"settled"}"#);
        let gongbu_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        gongbu_listener.set_nonblocking(true).unwrap();
        let gongbu_endpoint = format!("http://{}", gongbu_listener.local_addr().unwrap());
        let server = server_with_backends(
            &hubu_endpoint,
            Some(&gongbu_endpoint),
            true,
            Some("reconciliation-canary"),
        );
        let response = tool_call(
            &server,
            "hubu_reconcile_vendor_did_not_bill_claim",
            json!({"claim_id":"claim-1","provider_reference":"provider-1","evidence":"reviewed"}),
            None,
        );
        let raw = request.recv_timeout(Duration::from_secs(2)).unwrap();
        handle.join().unwrap();
        assert!(response.get("error").is_none());
        assert!(raw.contains("authorization: Bearer hubu-token-canary"));
        assert!(raw.contains("x-hubu-reconciliation-capability: reconciliation-canary"));
        assert!(!raw.contains("gongbu-token-canary"));
        assert!(
            matches!(gongbu_listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
        );
    }

    #[test]
    fn reconciliation_without_distinct_capability_fails_before_network() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let mut server = server_with_backends(&endpoint, None, true, None);
        server.hubu_routing.reconciliation_capability_file = format!(
            "/private/tmp/hubu-91-missing-reconciliation-token-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        );

        let response = tool_call(
            &server,
            "hubu_reconcile_vendor_did_not_bill_claim",
            json!({"claim_id":"claim-1","provider_reference":"provider-1","evidence":"reviewed"}),
            None,
        );
        assert_eq!(response["error"]["code"], -32000);
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("human reconciliation requires"));
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
        );
    }

    #[test]
    fn protected_and_unapproved_hubu_tools_fail_before_network() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = server_with_backends(&endpoint, Some(&endpoint), false, None);

        let protected = tool_call(&server, "hubu_create_budget", json!({}), None);
        assert_eq!(protected["error"]["code"], -32000);
        assert!(protected["error"]["message"]
            .as_str()
            .unwrap()
            .contains("trusted MCP client approval gate"));
        for name in [
            "hubu_get_spend_approval",
            "hubu_resolve_spend_approval",
            "hubu_not_a_tool",
        ] {
            let rejected = tool_call(&server, name, json!({}), None);
            assert_eq!(rejected["error"]["code"], -32602, "{name}");
        }
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
        );
    }

    #[test]
    fn hubu_outage_is_sanitized_retryable_and_has_no_fallback() {
        let unavailable = TcpListener::bind("127.0.0.1:0").unwrap();
        let hubu_endpoint = format!("http://{}", unavailable.local_addr().unwrap());
        drop(unavailable);
        let gongbu_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        gongbu_listener.set_nonblocking(true).unwrap();
        let gongbu_endpoint = format!("http://{}", gongbu_listener.local_addr().unwrap());
        let server = server_with_backends(&hubu_endpoint, Some(&gongbu_endpoint), false, None);

        let response = tool_call(&server, "hubu_health", json!({}), None);
        assert_eq!(response["error"]["code"], -32010);
        assert_eq!(response["error"]["data"]["code"], "backend_unavailable");
        assert_eq!(response["error"]["data"]["owner"], "hubu");
        assert_eq!(response["error"]["data"]["retryable"], true);
        let serialized = response.to_string();
        assert!(!serialized.contains("hubu-token-canary"));
        assert!(!serialized.contains(&hubu_endpoint));
        assert!(
            matches!(gongbu_listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
        );

        let capability = tool_call(&server, "hubu_unified_capabilities", json!({}), None);
        assert!(capability.get("error").is_none());
    }

    #[test]
    fn forwarded_application_errors_preserve_hubu_contract_without_secrets() {
        let (endpoint, _request, handle) = one_shot_http_server(
            403,
            r#"{"error":"bearer hubu-token-canary reconciliation reconciliation-canary"}"#,
        );
        let server = server_with_backends(&endpoint, None, true, Some("reconciliation-canary"));
        let response = tool_call(
            &server,
            "hubu_reconcile_vendor_did_not_bill_claim",
            json!({"claim_id":"claim-1","provider_reference":"provider-1","evidence":"reviewed"}),
            None,
        );
        handle.join().unwrap();
        assert_eq!(response["error"]["code"], -32000);
        assert_eq!(
            response["error"]["message"],
            "Hubu server returned HTTP 403: bearer <redacted> reconciliation <redacted>"
        );
        let serialized = response.to_string();
        assert!(!serialized.contains("hubu-token-canary"));
        assert!(!serialized.contains(&endpoint));
    }

    #[test]
    fn malformed_mutation_response_is_sanitized_ambiguous_and_not_retried() {
        let (endpoint, request, handle) = one_shot_http_server(200, "backend-secret-not-json");
        let server = server_with_backends(&endpoint, None, true, None);
        let response = tool_call(&server, "hubu_create_budget", json!({}), None);
        let raw = request.recv_timeout(Duration::from_secs(2)).unwrap();
        handle.join().unwrap();

        assert!(raw.starts_with("POST /budgets HTTP/1.1"));
        assert_eq!(response["error"]["code"], -32000);
        assert_eq!(
            response["error"]["message"],
            "Hubu backend request failed after dispatch; mutation outcome may be ambiguous"
        );
        assert!(!response.to_string().contains("backend-secret-not-json"));
    }

    #[test]
    fn connected_read_outage_is_retryable_and_never_reaches_gongbu() {
        let (hubu_endpoint, request, handle) = disconnect_after_request_server();
        let gongbu_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        gongbu_listener.set_nonblocking(true).unwrap();
        let gongbu_endpoint = format!("http://{}", gongbu_listener.local_addr().unwrap());
        let server = server_with_backends(&hubu_endpoint, Some(&gongbu_endpoint), false, None);

        let response = tool_call(&server, "hubu_health", json!({}), None);
        let raw = request.recv_timeout(Duration::from_secs(2)).unwrap();
        handle.join().unwrap();
        assert!(raw.starts_with("GET /health HTTP/1.1"));
        assert_eq!(response["error"]["code"], -32010);
        assert_eq!(response["error"]["data"]["code"], "backend_unavailable");
        assert_eq!(response["error"]["data"]["retryable"], true);
        assert!(
            matches!(gongbu_listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
        );
    }

    #[test]
    fn production_dependencies_exclude_backend_implementation_crates() {
        let unified_manifest = include_str!("../Cargo.toml");
        let hubu_adapter_manifest = include_str!("../../hubu-mcp/Cargo.toml");
        for forbidden in [
            "hubu-api",
            "hubu-core",
            "hubu-wallet",
            "gongbu-api",
            "gongbu-mcp",
        ] {
            assert!(!unified_manifest.contains(forbidden), "{forbidden}");
            assert!(!hubu_adapter_manifest.contains(forbidden), "{forbidden}");
        }
        assert!(unified_manifest.contains("hubu-mcp"));
    }

    #[test]
    fn eof_is_a_graceful_shutdown() {
        let server = Server::new(Config::default()).unwrap();
        let mut output = Vec::new();
        server
            .run(Cursor::new(Vec::<u8>::new()), &mut output)
            .unwrap();
        assert!(output.is_empty());
    }
}
