//! Unified MCP server for the separate Hubu and Gongbu services.
//!
//! This crate deliberately has no dependency on either backend's domain or
//! server crate. Backend clients hold independent endpoints, credentials, HTTP
//! clients, and failure boundaries. Both approved domain catalogs route through
//! public, versioned adapter contracts without importing backend implementation
//! crates.

mod gongbu;
mod hubu;

use std::{
    env, fmt,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
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
mod credential;
mod diagnostics;
mod notification;
mod operation_registry;
mod probe;
mod stdio;

use capability::{capabilities_value, CapabilitySnapshot};
use diagnostics::{backend_error_response, tool_availability, ToolRejection};
pub use hubu::RoutingConfig as HubuRoutingConfig;
use notification::TransitionState;

pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
pub const UNIFIED_CONTRACT_VERSION: &str = "hubu-gongbu-mcp-v1";
pub const EXECUTOR_CONTRACT_VERSION: &str = "hubu-spend-executor-v4.2";
pub const HUBU_ROUTING_CONTRACT_VERSION: &str = "hubu-mcp-routing-v1";
pub const ROUTING_REVISION: u32 = 1;

const HUBU_ENDPOINT_ENV: &str = "HUBU_UNIFIED_HUBU_ENDPOINT";
const HUBU_TOKEN_ENV: &str = "HUBU_UNIFIED_HUBU_BEARER_TOKEN";
const HUBU_TOKEN_FILE_ENV: &str = "HUBU_UNIFIED_HUBU_BEARER_TOKEN_FILE";
const GONGBU_ENDPOINT_ENV: &str = "HUBU_UNIFIED_GONGBU_ENDPOINT";
const GONGBU_TOKEN_ENV: &str = "HUBU_UNIFIED_GONGBU_BEARER_TOKEN";
const GONGBU_TOKEN_FILE_ENV: &str = "HUBU_UNIFIED_GONGBU_BEARER_TOKEN_FILE";
const CAPABILITY_POLL_INTERVAL_ENV: &str = "HUBU_UNIFIED_CAPABILITY_POLL_INTERVAL_MS";
const OPERATION_STATE_PATH_ENV: &str = "HUBU_UNIFIED_OPERATION_STATE_PATH";
const TRUST_CLIENT_APPROVAL_ENV: &str = "HUBU_MCP_TRUST_CLIENT_APPROVAL";
const RECONCILIATION_TOKEN_ENV: &str = "HUBU_RECONCILIATION_TOKEN";
const RECONCILIATION_TOKEN_FILE_ENV: &str = "HUBU_RECONCILIATION_TOKEN_FILE";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const DEFAULT_CAPABILITY_POLL_INTERVAL: Duration = Duration::from_secs(30);
const MIN_CAPABILITY_POLL_INTERVAL_MS: u64 = 10;
const MAX_CAPABILITY_POLL_INTERVAL_MS: u64 = 60_000;
const MAX_CAPABILITY_FAILURE_BACKOFF: Duration = Duration::from_secs(5 * 60);

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

#[derive(Clone, Debug)]
pub struct Config {
    pub hubu: Option<BackendConfig>,
    pub gongbu: Option<BackendConfig>,
    pub hubu_routing: HubuRoutingConfig,
    pub capability_poll_interval: Duration,
    pub operation_state_path: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hubu: None,
            gongbu: None,
            hubu_routing: HubuRoutingConfig::default(),
            capability_poll_interval: DEFAULT_CAPABILITY_POLL_INTERVAL,
            operation_state_path: None,
        }
    }
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let hubu_token =
            credential::from_env(BackendOwner::Hubu, HUBU_TOKEN_ENV, HUBU_TOKEN_FILE_ENV)?;
        let gongbu_token = credential::from_env(
            BackendOwner::Gongbu,
            GONGBU_TOKEN_ENV,
            GONGBU_TOKEN_FILE_ENV,
        )?;
        Self::from_lookup(|name| match name {
            HUBU_TOKEN_ENV => hubu_token.clone(),
            GONGBU_TOKEN_ENV => gongbu_token.clone(),
            _ => env::var(name).ok(),
        })
    }

    fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let operation_state_path = lookup(OPERATION_STATE_PATH_ENV)
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from);
        let mut hubu_routing = HubuRoutingConfig::new(
            lookup(TRUST_CLIENT_APPROVAL_ENV).is_some_and(|value| env_flag_value(&value)),
            lookup(RECONCILIATION_TOKEN_ENV),
        );
        hubu_routing.reconciliation_capability_file = lookup(RECONCILIATION_TOKEN_FILE_ENV)
            .unwrap_or_else(|| "hubu.reconciliation-token".to_string());
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
            capability_poll_interval: poll_interval(lookup(CAPABILITY_POLL_INTERVAL_ENV))?,
            operation_state_path,
        })
    }
}

fn poll_interval(value: Option<String>) -> Result<Duration, ConfigError> {
    let Some(value) = value else {
        return Ok(DEFAULT_CAPABILITY_POLL_INTERVAL);
    };
    let milliseconds = value
        .parse::<u64>()
        .map_err(|_| ConfigError::InvalidPollInterval)?;
    if !(MIN_CAPABILITY_POLL_INTERVAL_MS..=MAX_CAPABILITY_POLL_INTERVAL_MS).contains(&milliseconds)
    {
        return Err(ConfigError::InvalidPollInterval);
    }
    Ok(Duration::from_millis(milliseconds))
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
    #[error("{0} backend credential file could not be read or was empty")]
    CredentialFile(BackendOwner),
    #[error(
        "{0} backend endpoint must be an HTTP(S) base URL without credentials, query, or fragment"
    )]
    InvalidEndpoint(BackendOwner),
    #[error("capability poll interval must be between 10 and 60000 milliseconds")]
    InvalidPollInterval,
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
    hubu_probe_gate: Arc<probe::ProbeGate>,
    gongbu_probe_gate: Arc<probe::ProbeGate>,
}

impl BackendClients {
    pub fn new(config: Config) -> Result<Self, ConfigError> {
        Ok(Self {
            hubu: config.hubu.map(BackendClient::new).transpose()?,
            gongbu: config.gongbu.map(BackendClient::new).transpose()?,
            hubu_probe_gate: Arc::new(probe::ProbeGate::default()),
            gongbu_probe_gate: Arc::new(probe::ProbeGate::default()),
        })
    }
}

#[derive(Clone, Debug)]
pub struct Server {
    backends: BackendClients,
    snapshot: Arc<Mutex<CapabilitySnapshot>>,
    transition_state: Arc<TransitionState>,
    capability_poll_interval: Duration,
    probe_timings: Arc<Mutex<ProbeTimings>>,
    probe_schedule_waker: Arc<Mutex<Option<mpsc::Sender<()>>>>,
    hubu_routing: HubuRoutingConfig,
    operation_registry: Arc<OperationRegistryCapability>,
}

#[derive(Debug)]
enum OperationRegistryCapability {
    Available(Mutex<operation_registry::OperationRegistry>),
    Unavailable { reason_code: &'static str },
}

#[derive(Clone, Copy, Debug)]
struct ProbeTimings {
    hubu: BackendProbeTiming,
    gongbu: BackendProbeTiming,
}

#[derive(Clone, Copy, Debug)]
struct BackendProbeTiming {
    next_probe_at: Instant,
    failure_streak: u32,
    jitter_state: u64,
}

impl BackendProbeTiming {
    fn new(now: Instant, base: Duration, failed: bool, jitter_seed: u64) -> Self {
        let mut timing = Self {
            next_probe_at: now,
            failure_streak: 0,
            jitter_state: jitter_seed.max(1),
        };
        timing.record_result(now, base, failed);
        timing
    }

    fn record_result(&mut self, now: Instant, base: Duration, failed: bool) {
        let multiplier = if failed {
            let multiplier = 1_u32 << self.failure_streak.min(31);
            self.failure_streak = self.failure_streak.saturating_add(1);
            multiplier
        } else {
            self.failure_streak = 0;
            1
        };
        let backed_off = base
            .saturating_mul(multiplier)
            .min(MAX_CAPABILITY_FAILURE_BACKOFF);
        let jitter_percent = 80 + self.next_jitter_value() % 41;
        let millis = backed_off
            .as_millis()
            .saturating_mul(u128::from(jitter_percent))
            / 100;
        let delay = Duration::from_millis(
            u64::try_from(millis.max(1))
                .unwrap_or(u64::MAX)
                .min(MAX_CAPABILITY_FAILURE_BACKOFF.as_millis() as u64),
        );
        self.next_probe_at = now + delay;
    }

    fn claim_if_due(&mut self, now: Instant, base: Duration) -> bool {
        if now < self.next_probe_at {
            return false;
        }
        self.next_probe_at = now + base;
        true
    }

    fn delay_from(self, now: Instant) -> Duration {
        self.next_probe_at.saturating_duration_since(now)
    }

    fn next_jitter_value(&mut self) -> u64 {
        self.jitter_state ^= self.jitter_state << 13;
        self.jitter_state ^= self.jitter_state >> 7;
        self.jitter_state ^= self.jitter_state << 17;
        self.jitter_state
    }
}

impl Server {
    pub fn new(config: Config) -> Result<Self, ConfigError> {
        let operation_registry = match config.operation_state_path.as_deref() {
            Some(path) if path == Path::new(":memory:") => {
                OperationRegistryCapability::Unavailable {
                    reason_code: "configuration_invalid",
                }
            }
            Some(path) => match operation_registry::OperationRegistry::open(path) {
                Ok(registry) => OperationRegistryCapability::Available(Mutex::new(registry)),
                Err(_) => OperationRegistryCapability::Unavailable {
                    reason_code: "state_unavailable",
                },
            },
            None => OperationRegistryCapability::Unavailable {
                reason_code: "configuration_missing",
            },
        };
        let hubu_routing = config.hubu_routing.clone();
        let capability_poll_interval = config.capability_poll_interval;
        let backends = BackendClients::new(config)?;
        let snapshot = backends.probe();
        let probed_at = Instant::now();
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64
            ^ u64::from(std::process::id());
        let probe_timings = ProbeTimings {
            hubu: BackendProbeTiming::new(
                probed_at,
                capability_poll_interval,
                report_unavailable(&snapshot.hubu),
                seed ^ 0x4855_4255,
            ),
            gongbu: BackendProbeTiming::new(
                probed_at,
                capability_poll_interval,
                report_unavailable(&snapshot.gongbu),
                seed ^ 0x474f_4e47_4255,
            ),
        };
        let transition_state = TransitionState::new(&snapshot);
        Ok(Self {
            backends,
            snapshot: Arc::new(Mutex::new(snapshot)),
            transition_state: Arc::new(transition_state),
            capability_poll_interval,
            probe_timings: Arc::new(Mutex::new(probe_timings)),
            probe_schedule_waker: Arc::new(Mutex::new(None)),
            hubu_routing,
            operation_registry: Arc::new(operation_registry),
        })
    }

    pub fn run(self, input: impl BufRead + Send, output: impl Write) -> io::Result<()> {
        stdio::run(self, input, output)
    }

    pub(crate) fn handle_line(&self, line: &str) -> Option<Value> {
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
            if call.name == "gongbu_create_execution" {
                // Governed execution admission must synchronously validate both
                // backend boundaries immediately before forwarding.
                self.refresh_capabilities();
            } else {
                self.refresh_gongbu_capability_if_stale();
            }
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
        self.refresh_hubu_capability_if_stale();
        hubu::call_tool(self, id, call)
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
        hubu::call_tool(self, id, call)
    }

    fn list_tools(&self) -> Vec<Value> {
        self.refresh_capabilities_if_stale();
        self.list_tools_for_snapshot()
    }

    fn list_tools_for_snapshot(&self) -> Vec<Value> {
        let snapshot = self.snapshot();
        let mut tools = vec![capability_tool()];
        if tool_availability("hubu_health", BackendOwner::Hubu, &snapshot).is_ok() {
            tools.extend(hubu::tool_definitions().into_iter().filter(|tool| {
                self.operation_registry_available()
                    || !matches!(
                        tool["name"].as_str(),
                        Some("hubu_authorize_spend" | "hubu_submit_spend")
                    )
            }));
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
        let mut capability = capabilities_value(&self.snapshot());
        let (available, reason_code) = match self.operation_registry.as_ref() {
            OperationRegistryCapability::Available(_) => (true, None),
            OperationRegistryCapability::Unavailable { reason_code } => (false, Some(*reason_code)),
        };
        capability["operation_registry"] = json!({
            "state": if available { "available" } else { "unavailable" },
            "reason_code": reason_code,
            "billable_operations_available": available
        });
        if !available {
            for tool in capability["tools"]
                .as_array_mut()
                .expect("capability tools are an array")
            {
                if matches!(
                    tool["name"].as_str(),
                    Some("hubu_authorize_spend" | "hubu_submit_spend")
                ) {
                    tool["available"] = json!(false);
                    tool["reason_code"] = json!("operation_registry_unavailable");
                }
            }
        }
        capability
    }

    fn operation_registry_available(&self) -> bool {
        matches!(
            self.operation_registry.as_ref(),
            OperationRegistryCapability::Available(_)
        )
    }

    fn resolve_harness_operation(
        &self,
        identity: &operation_registry::NormalizedHarnessIdentity,
        tool_name: &str,
        arguments: &Value,
    ) -> anyhow::Result<operation_registry::OperationResolution> {
        let OperationRegistryCapability::Available(registry) = self.operation_registry.as_ref()
        else {
            anyhow::bail!(
                "Hubu billable tools require an available operation registry; configure {OPERATION_STATE_PATH_ENV} to an absolute writable SQLite file"
            );
        };
        registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .resolve_or_allocate(identity, tool_name, arguments)
    }

    fn snapshot(&self) -> CapabilitySnapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn refresh_capabilities(&self) {
        let probe_id = self.transition_state.next_probe_id();
        let refreshed = self.backends.probe();
        let hubu_failed = report_unavailable(&refreshed.hubu);
        let gongbu_failed = report_unavailable(&refreshed.gongbu);
        self.transition_state
            .apply_full(&self.snapshot, probe_id, refreshed);
        let now = Instant::now();
        let mut timings = self
            .probe_timings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        timings
            .hubu
            .record_result(now, self.capability_poll_interval, hubu_failed);
        timings
            .gongbu
            .record_result(now, self.capability_poll_interval, gongbu_failed);
        drop(timings);
        self.wake_probe_monitor();
    }

    fn refresh_hubu_capability(&self) {
        let probe_id = self.transition_state.next_probe_id();
        let refreshed = self.backends.probe_hubu();
        let failed = report_unavailable(&refreshed);
        self.transition_state
            .apply_hubu(&self.snapshot, probe_id, refreshed);
        self.probe_timings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .hubu
            .record_result(Instant::now(), self.capability_poll_interval, failed);
        self.wake_probe_monitor();
    }

    fn refresh_gongbu_capability(&self) {
        let probe_id = self.transition_state.next_probe_id();
        let refreshed = self.backends.probe_gongbu();
        let failed = report_unavailable(&refreshed);
        self.transition_state
            .apply_gongbu(&self.snapshot, probe_id, refreshed);
        self.probe_timings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .gongbu
            .record_result(Instant::now(), self.capability_poll_interval, failed);
        self.wake_probe_monitor();
    }

    fn refresh_capabilities_if_stale(&self) {
        let now = Instant::now();
        let mut timings = self
            .probe_timings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let hubu_due = timings
            .hubu
            .claim_if_due(now, self.capability_poll_interval);
        let gongbu_due = timings
            .gongbu
            .claim_if_due(now, self.capability_poll_interval);
        drop(timings);
        match (hubu_due, gongbu_due) {
            (true, true) => self.refresh_capabilities(),
            (true, false) => self.refresh_hubu_capability(),
            (false, true) => self.refresh_gongbu_capability(),
            (false, false) => {}
        }
    }

    fn refresh_hubu_capability_if_stale(&self) {
        let due = self
            .probe_timings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .hubu
            .claim_if_due(Instant::now(), self.capability_poll_interval);
        if due {
            self.refresh_hubu_capability();
        }
    }

    fn refresh_gongbu_capability_if_stale(&self) {
        let due = self
            .probe_timings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .gongbu
            .claim_if_due(Instant::now(), self.capability_poll_interval);
        if due {
            self.refresh_gongbu_capability();
        }
    }

    pub(crate) fn next_capability_probe_delay(&self) -> Duration {
        let now = Instant::now();
        let timings = *self
            .probe_timings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        timings
            .hubu
            .delay_from(now)
            .min(timings.gongbu.delay_from(now))
    }

    pub(crate) fn refresh_due_capabilities(&self) {
        self.refresh_capabilities_if_stale();
    }

    pub(crate) fn install_probe_schedule_waker(&self, waker: mpsc::Sender<()>) {
        *self
            .probe_schedule_waker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(waker);
    }

    fn wake_probe_monitor(&self) {
        let waker = self
            .probe_schedule_waker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(waker) = waker {
            let _ = waker.send(());
        }
    }

    fn mark_hubu_unavailable(&self) {
        let probe_id = self.transition_state.next_probe_id();
        self.transition_state
            .mark_hubu_unavailable(&self.snapshot, probe_id);
        self.probe_timings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .hubu
            .record_result(Instant::now(), self.capability_poll_interval, true);
        self.wake_probe_monitor();
    }

    pub(crate) fn take_pending_catalog_transitions(&self) -> usize {
        self.transition_state.take_pending()
    }

    pub(crate) fn reset_catalog_tracking(&self) {
        let snapshot = self.snapshot();
        self.transition_state.reset(&snapshot);
    }
}

fn report_unavailable(report: &capability::BackendReport) -> bool {
    matches!(report.state, capability::BackendState::Unavailable)
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

pub fn run_stdio_from_env(
    input: impl BufRead + Send,
    output: impl Write,
) -> Result<(), StartupError> {
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
            (OPERATION_STATE_PATH_ENV, "/tmp/hubu-unified-test.sqlite3"),
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
    fn validates_injectable_capability_poll_interval() {
        assert_eq!(poll_interval(None).unwrap(), Duration::from_secs(30));
        assert_eq!(
            poll_interval(Some("10".into())).unwrap(),
            Duration::from_millis(10)
        );
        assert_eq!(
            poll_interval(Some("60000".into())).unwrap(),
            Duration::from_secs(60)
        );
        assert_eq!(
            poll_interval(Some("9".into())).unwrap_err(),
            ConfigError::InvalidPollInterval
        );
        assert_eq!(
            poll_interval(Some("secret/path".into())).unwrap_err(),
            ConfigError::InvalidPollInterval
        );
    }

    #[test]
    fn missing_operation_registry_preserves_non_billable_startup() {
        let server = Server::new(Config::from_lookup(lookup(&[])).unwrap()).unwrap();
        let capability = server.capabilities();
        assert_eq!(capability["operation_registry"]["state"], "unavailable");
        assert_eq!(
            capability["operation_registry"]["reason_code"],
            "configuration_missing"
        );
        assert_eq!(server.list_tools_for_snapshot().len(), 1);
    }

    #[test]
    fn broken_operation_registry_degrades_billable_capability_without_startup_failure() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state-directory");
        std::fs::create_dir(&path).unwrap();
        let server = Server::new(Config {
            operation_state_path: Some(path),
            ..Config::default()
        })
        .unwrap();
        let capability = server.capabilities();
        assert_eq!(capability["operation_registry"]["state"], "unavailable");
        assert_eq!(
            capability["operation_registry"]["reason_code"],
            "state_unavailable"
        );
    }

    #[test]
    fn in_memory_operation_registry_is_rejected_for_runtime_configuration() {
        let server = Server::new(Config {
            operation_state_path: Some(PathBuf::from(":memory:")),
            ..Config::default()
        })
        .unwrap();
        let capability = server.capabilities();
        assert_eq!(capability["operation_registry"]["state"], "unavailable");
        assert_eq!(
            capability["operation_registry"]["reason_code"],
            "configuration_invalid"
        );
    }

    #[test]
    fn probe_timing_jitters_backs_off_and_resets_after_recovery() {
        let now = Instant::now();
        let base = Duration::from_secs(30);
        let mut timing = BackendProbeTiming::new(now, base, true, 11);
        let first = timing.next_probe_at.duration_since(now);
        timing.record_result(now, base, true);
        let second = timing.next_probe_at.duration_since(now);
        timing.record_result(now, base, true);
        let third = timing.next_probe_at.duration_since(now);

        assert!((Duration::from_secs(24)..=Duration::from_secs(36)).contains(&first));
        assert!(second > first);
        assert!(third > second);

        timing.record_result(now, base, false);
        let recovered = timing.next_probe_at.duration_since(now);
        assert!((Duration::from_secs(24)..=Duration::from_secs(36)).contains(&recovered));
    }

    #[test]
    fn short_poll_interval_backoff_reaches_the_configured_cap() {
        let now = Instant::now();
        let base = Duration::from_secs(1);
        let mut timing = BackendProbeTiming::new(now, base, true, 19);
        for _ in 0..9 {
            timing.record_result(now, base, true);
        }

        let capped = timing.next_probe_at.duration_since(now);
        assert!((Duration::from_secs(240)..=Duration::from_secs(300)).contains(&capped));
    }

    #[test]
    fn forced_refresh_wakes_the_probe_monitor() {
        let server = Server::new(Config::default()).unwrap();
        let (wake_tx, wake_rx) = mpsc::channel();
        server.install_probe_schedule_waker(wake_tx);

        server.refresh_capabilities();

        wake_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("forced refresh must wake the probe monitor");
    }

    #[test]
    fn backend_probe_backoff_is_independent() {
        let now = Instant::now();
        let base = Duration::from_secs(30);
        let mut hubu = BackendProbeTiming::new(now, base, true, 13);
        let mut gongbu = BackendProbeTiming::new(now, base, false, 17);
        hubu.record_result(now, base, true);
        gongbu.record_result(now, base, false);

        assert!(hubu.next_probe_at.duration_since(now) > gongbu.next_probe_at.duration_since(now));
    }

    #[test]
    fn incomplete_pair_is_unconfigured_without_blocking_the_other_backend() {
        let config = Config::from_lookup(lookup(&[
            (OPERATION_STATE_PATH_ENV, "/tmp/hubu-unified-test.sqlite3"),
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
                (OPERATION_STATE_PATH_ENV, "/tmp/hubu-unified-test.sqlite3"),
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
            ..Config::default()
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
