//! Unified MCP transport shell for the separate Hubu and Gongbu services.
//!
//! This crate deliberately has no dependency on either backend's domain or
//! server crate. Backend clients hold independent endpoints, credentials, HTTP
//! clients, and failure boundaries. Domain tool catalogs and forwarding are
//! implemented by follow-up issues.

use std::{
    env, fmt,
    io::{self, BufRead, Write},
    sync::{Arc, Mutex},
    time::Duration,
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

use capability::{capabilities_value, CapabilitySnapshot};
use diagnostics::{backend_error_response, tool_availability};

pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
pub const UNIFIED_CONTRACT_VERSION: &str = "hubu-gongbu-mcp-v1";
pub const EXECUTOR_CONTRACT_VERSION: &str = "hubu-spend-executor-v4.2";
pub const ROUTING_REVISION: u32 = 1;

const HUBU_ENDPOINT_ENV: &str = "HUBU_UNIFIED_HUBU_ENDPOINT";
const HUBU_TOKEN_ENV: &str = "HUBU_UNIFIED_HUBU_BEARER_TOKEN";
const GONGBU_ENDPOINT_ENV: &str = "HUBU_UNIFIED_GONGBU_ENDPOINT";
const GONGBU_TOKEN_ENV: &str = "HUBU_UNIFIED_GONGBU_BEARER_TOKEN";
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

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub hubu: Option<BackendConfig>,
    pub gongbu: Option<BackendConfig>,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| env::var(name).ok())
    }

    fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Result<Self, ConfigError> {
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
        })
    }
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
    http: Client,
}

impl fmt::Debug for BackendClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BackendClient")
            .field("owner", &self.owner)
            .field("endpoint", &self.endpoint)
            .field("executor_contract", &EXECUTOR_CONTRACT_VERSION)
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
}

impl Server {
    pub fn new(config: Config) -> Result<Self, ConfigError> {
        let backends = BackendClients::new(config)?;
        let snapshot = backends.probe();
        Ok(Self {
            backends,
            snapshot: Arc::new(Mutex::new(snapshot)),
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
            "tools/list" => success_response(id, json!({ "tools": [capability_tool()] })),
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
        let call: ToolCall = match serde_json::from_value(params) {
            Ok(call) => call,
            Err(_) => return error_response(id, -32602, "Invalid params"),
        };
        // Parsed at the shared boundary so future routed tools can consume
        // trusted platform metadata without placing it in model arguments.
        let _trusted_meta = &call.meta;
        if call.arguments.as_object().is_none() {
            return error_response(id, -32602, "Invalid params");
        }
        self.refresh_capabilities();
        if call.name == "hubu_unified_capabilities" {
            if call
                .arguments
                .as_object()
                .is_some_and(|arguments| !arguments.is_empty())
            {
                return error_response(id, -32602, "Invalid params");
            }
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
        error_response(
            id,
            -32601,
            "Domain tool routing is not implemented by this capability release",
        )
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
    use std::io::Cursor;

    fn lookup<'a>(values: &'a [(&'a str, &'a str)]) -> impl FnMut(&str) -> Option<String> + 'a {
        move |name| {
            values
                .iter()
                .find_map(|(key, value)| (*key == name).then(|| (*value).to_owned()))
        }
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
    fn eof_is_a_graceful_shutdown() {
        let server = Server::new(Config::default()).unwrap();
        let mut output = Vec::new();
        server
            .run(Cursor::new(Vec::<u8>::new()), &mut output)
            .unwrap();
        assert!(output.is_empty());
    }
}
