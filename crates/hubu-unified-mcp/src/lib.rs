//! Unified MCP transport shell for the separate Hubu and Gongbu services.
//!
//! This crate deliberately has no dependency on either backend's domain or
//! server crate. Backend clients hold independent endpoints, credentials, HTTP
//! clients, and failure boundaries. Domain tool catalogs and forwarding are
//! implemented by follow-up issues.

use std::{
    collections::BTreeMap,
    env, fmt,
    io::{self, BufRead, Write},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use chrono::{SecondsFormat, Utc};
use reqwest::{
    blocking::Client,
    header::{self, HeaderMap, HeaderValue},
    Url,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

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
const GONGBU_API_SCHEMA_VERSION: u32 = 2;
const GONGBU_MCP_SCHEMA_VERSION: u32 = 2;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum BackendState {
    Available,
    Degraded,
    Unavailable,
    Incompatible,
    Unconfigured,
}

#[derive(Clone, Debug, Serialize)]
struct ContractVersions {
    executor: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct BackendReport {
    state: BackendState,
    product_version: Option<String>,
    source_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_schema_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mcp_schema_version: Option<u32>,
    contract_versions: ContractVersions,
    reason_code: Option<&'static str>,
}

impl BackendReport {
    fn unconfigured() -> Self {
        Self {
            state: BackendState::Unconfigured,
            product_version: None,
            source_commit: None,
            api_schema_version: None,
            mcp_schema_version: None,
            contract_versions: ContractVersions { executor: None },
            reason_code: Some("configuration_missing"),
        }
    }
}

#[derive(Clone, Debug)]
struct CapabilitySnapshot {
    generated_at: String,
    hubu: BackendReport,
    gongbu: BackendReport,
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

#[derive(Debug)]
struct ProbeResponse {
    status: u16,
    body: Value,
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

    fn probe(&self, path: &str) -> Result<ProbeResponse, ()> {
        let url = self.endpoint.join(path).map_err(|_| ())?;
        let response = self.http.get(url).send().map_err(|_| ())?;
        let status = response.status().as_u16();
        let body = response.json().map_err(|_| ())?;
        Ok(ProbeResponse { status, body })
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

    fn probe(&self) -> CapabilitySnapshot {
        thread::scope(|scope| {
            let hubu = scope.spawn(|| {
                self.hubu
                    .as_ref()
                    .map(probe_hubu)
                    .unwrap_or_else(BackendReport::unconfigured)
            });
            let gongbu = scope.spawn(|| {
                self.gongbu
                    .as_ref()
                    .map(probe_gongbu)
                    .unwrap_or_else(BackendReport::unconfigured)
            });
            CapabilitySnapshot {
                generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                hubu: hubu.join().expect("Hubu capability probe must not panic"),
                gongbu: gongbu
                    .join()
                    .expect("Gongbu capability probe must not panic"),
            }
        })
    }
}

fn probe_hubu(client: &BackendClient) -> BackendReport {
    let health = client.probe("health");
    let version = client.probe("version");
    classify_hubu(health.as_ref().ok(), version.as_ref().ok())
}

fn classify_hubu(health: Option<&ProbeResponse>, version: Option<&ProbeResponse>) -> BackendReport {
    classify_hubu_for(health, version, product_version(), source_commit())
}

fn classify_hubu_for(
    health: Option<&ProbeResponse>,
    version: Option<&ProbeResponse>,
    expected_product_version: &str,
    expected_source_commit: &str,
) -> BackendReport {
    let metadata = version.and_then(|response| response.body.as_object());
    let reported_product_version = metadata
        .and_then(|value| value.get("product_version"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let source = metadata
        .and_then(|value| value.get("source_commit"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let executor = metadata
        .and_then(|value| value.get("executor_contract"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut report = BackendReport {
        state: BackendState::Unavailable,
        product_version: reported_product_version,
        source_commit: source,
        api_schema_version: None,
        mcp_schema_version: None,
        contract_versions: ContractVersions { executor },
        reason_code: Some("health_unavailable"),
    };

    if !health.is_some_and(|response| {
        (200..300).contains(&response.status) && response.body["status"] == "ok"
    }) {
        return report;
    }
    if !version.is_some_and(|response| (200..300).contains(&response.status)) {
        report.reason_code = Some("version_unavailable");
        return report;
    }
    if report.product_version.as_deref() != Some(expected_product_version) {
        return incompatible(report, "product_version_mismatch");
    }
    if !matching_source_commit(report.source_commit.as_deref(), expected_source_commit) {
        return incompatible(report, "source_commit_mismatch");
    }
    if report.contract_versions.executor.as_deref() != Some(EXECUTOR_CONTRACT_VERSION) {
        return incompatible(report, "executor_contract_mismatch");
    }
    report.state = BackendState::Available;
    report.reason_code = None;
    report
}

fn probe_gongbu(client: &BackendClient) -> BackendReport {
    let live = client.probe("livez");
    let ready = client.probe("readyz");
    let version = client.probe("version");
    classify_gongbu(
        live.as_ref().ok(),
        ready.as_ref().ok(),
        version.as_ref().ok(),
    )
}

fn classify_gongbu(
    live: Option<&ProbeResponse>,
    ready: Option<&ProbeResponse>,
    version: Option<&ProbeResponse>,
) -> BackendReport {
    classify_gongbu_for(live, ready, version, product_version(), source_commit())
}

fn classify_gongbu_for(
    live: Option<&ProbeResponse>,
    ready: Option<&ProbeResponse>,
    version: Option<&ProbeResponse>,
    expected_product_version: &str,
    expected_source_commit: &str,
) -> BackendReport {
    let metadata = version.and_then(|response| response.body.as_object());
    let reported_product_version = string_value(metadata, "product_version");
    let source = string_value(metadata, "source_commit");
    let executor = string_value(metadata, "hubu_executor_contract");
    let api_schema_version = integer_value(metadata, "api_schema_version");
    let mcp_schema_version = integer_value(metadata, "mcp_schema_version");
    let mcp_protocol = string_value(metadata, "mcp_protocol_version");
    let mut report = BackendReport {
        state: BackendState::Unavailable,
        product_version: reported_product_version,
        source_commit: source,
        api_schema_version,
        mcp_schema_version,
        contract_versions: ContractVersions { executor },
        reason_code: Some("liveness_unavailable"),
    };

    if !live.is_some_and(|response| {
        (200..300).contains(&response.status) && response.body["status"] == "live"
    }) {
        return report;
    }
    if !version.is_some_and(|response| (200..300).contains(&response.status)) {
        report.reason_code = Some("version_unavailable");
        return report;
    }
    if report.product_version.as_deref() != Some(expected_product_version) {
        return incompatible(report, "product_version_mismatch");
    }
    if !matching_source_commit(report.source_commit.as_deref(), expected_source_commit) {
        return incompatible(report, "source_commit_mismatch");
    }
    if report.contract_versions.executor.as_deref() != Some(EXECUTOR_CONTRACT_VERSION) {
        return incompatible(report, "executor_contract_mismatch");
    }
    if report.api_schema_version != Some(GONGBU_API_SCHEMA_VERSION) {
        return incompatible(report, "api_schema_version_mismatch");
    }
    if report.mcp_schema_version != Some(GONGBU_MCP_SCHEMA_VERSION) {
        return incompatible(report, "mcp_schema_version_mismatch");
    }
    if mcp_protocol.as_deref() != Some(MCP_PROTOCOL_VERSION) {
        return incompatible(report, "mcp_protocol_version_mismatch");
    }
    if !ready.is_some_and(|response| {
        (200..300).contains(&response.status) && response.body["status"] == "ready"
    }) {
        report.state = BackendState::Degraded;
        report.reason_code = Some("backend_not_ready");
        return report;
    }
    report.state = BackendState::Available;
    report.reason_code = None;
    report
}

fn string_value(object: Option<&serde_json::Map<String, Value>>, key: &str) -> Option<String> {
    object?.get(key)?.as_str().map(str::to_owned)
}

fn integer_value(object: Option<&serde_json::Map<String, Value>>, key: &str) -> Option<u32> {
    object?.get(key)?.as_u64()?.try_into().ok()
}

fn incompatible(mut report: BackendReport, reason: &'static str) -> BackendReport {
    report.state = BackendState::Incompatible;
    report.reason_code = Some(reason);
    report
}

fn matching_source_commit(candidate: Option<&str>, expected: &str) -> bool {
    valid_source_commit(expected) && candidate == Some(expected)
}

fn valid_source_commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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

fn capabilities_value(snapshot: &CapabilitySnapshot) -> Value {
    let mut tools = DOMAIN_TOOLS
        .iter()
        .map(|(name, owner)| {
            let availability = tool_availability(name, *owner, snapshot);
            json!({
                "name": name,
                "owner": owner.as_str(),
                "available": availability.is_ok(),
                "reason_code": availability.err().map(ToolRejection::reason_code)
            })
        })
        .collect::<Vec<_>>();
    tools.push(json!({
        "name": "hubu_unified_capabilities",
        "owner": "router",
        "available": true,
        "reason_code": null
    }));
    tools.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));

    json!({
        "contract_version": UNIFIED_CONTRACT_VERSION,
        "routing_revision": ROUTING_REVISION,
        "generated_at": snapshot.generated_at,
        "backends": {
            "hubu": backend_value(&snapshot.hubu, false),
            "gongbu": backend_value(&snapshot.gongbu, true)
        },
        "tools": tools
    })
}

fn backend_value(report: &BackendReport, gongbu: bool) -> Value {
    let mut backend = BTreeMap::from([
        ("state", json!(report.state)),
        ("product_version", json!(report.product_version)),
        ("source_commit", json!(report.source_commit)),
        (
            "contract_versions",
            json!({ "executor": report.contract_versions.executor }),
        ),
        ("reason_code", json!(report.reason_code)),
    ]);
    if gongbu {
        backend.insert("api_schema_version", json!(report.api_schema_version));
        backend.insert("mcp_schema_version", json!(report.mcp_schema_version));
    }
    serde_json::to_value(backend).expect("backend placeholder serializes")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolRejection {
    Unconfigured,
    Unavailable,
    Incompatible,
    NotReady,
    RequiredBackendUnavailable,
}

impl ToolRejection {
    fn reason_code(self) -> &'static str {
        match self {
            Self::Unconfigured => "backend_unconfigured",
            Self::Unavailable | Self::RequiredBackendUnavailable => "backend_unavailable",
            Self::Incompatible => "backend_incompatible",
            Self::NotReady => "backend_not_ready",
        }
    }

    fn retryable(self) -> bool {
        matches!(
            self,
            Self::Unavailable | Self::NotReady | Self::RequiredBackendUnavailable
        )
    }
}

fn tool_availability(
    name: &str,
    owner: BackendOwner,
    snapshot: &CapabilitySnapshot,
) -> Result<(), ToolRejection> {
    let report = match owner {
        BackendOwner::Hubu => &snapshot.hubu,
        BackendOwner::Gongbu => &snapshot.gongbu,
    };
    match report.state {
        BackendState::Unconfigured => return Err(ToolRejection::Unconfigured),
        BackendState::Unavailable => return Err(ToolRejection::Unavailable),
        BackendState::Incompatible => return Err(ToolRejection::Incompatible),
        BackendState::Degraded if name == "gongbu_create_execution" => {
            return Err(ToolRejection::NotReady);
        }
        BackendState::Degraded | BackendState::Available => {}
    }
    if name == "gongbu_create_execution" && snapshot.hubu.state != BackendState::Available {
        return Err(match snapshot.hubu.state {
            BackendState::Unconfigured => ToolRejection::Unconfigured,
            BackendState::Incompatible => ToolRejection::Incompatible,
            BackendState::Degraded | BackendState::Unavailable => {
                ToolRejection::RequiredBackendUnavailable
            }
            BackendState::Available => unreachable!(),
        });
    }
    Ok(())
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

fn backend_error_response(
    id: Value,
    tool: &str,
    owner: BackendOwner,
    rejection: ToolRejection,
) -> Value {
    let code = rejection.reason_code();
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32010,
            "message": format!("{owner} backend cannot safely serve `{tool}` ({code})"),
            "data": {
                "code": code,
                "tool": tool,
                "owner": owner.as_str(),
                "retryable": rejection.retryable(),
                "capabilities_changed": true
            }
        }
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

    fn report(state: BackendState, gongbu: bool) -> BackendReport {
        BackendReport {
            state,
            product_version: Some("1.2.3".into()),
            source_commit: Some("a".repeat(40)),
            api_schema_version: gongbu.then_some(GONGBU_API_SCHEMA_VERSION),
            mcp_schema_version: gongbu.then_some(GONGBU_MCP_SCHEMA_VERSION),
            contract_versions: ContractVersions {
                executor: Some(EXECUTOR_CONTRACT_VERSION.into()),
            },
            reason_code: (state != BackendState::Available).then_some("test_state"),
        }
    }

    fn snapshot(hubu: BackendState, gongbu: BackendState) -> CapabilitySnapshot {
        CapabilitySnapshot {
            generated_at: "2026-08-18T00:00:00.000Z".into(),
            hubu: report(hubu, false),
            gongbu: report(gongbu, true),
        }
    }

    fn probe(status: u16, body: Value) -> ProbeResponse {
        ProbeResponse { status, body }
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
    fn capability_schema_covers_full_partial_unavailable_and_incompatible_states() {
        let cases = [
            (BackendState::Available, BackendState::Available),
            (BackendState::Available, BackendState::Unavailable),
            (BackendState::Unavailable, BackendState::Unavailable),
            (BackendState::Incompatible, BackendState::Available),
        ];
        for (hubu, gongbu) in cases {
            let capability = capabilities_value(&snapshot(hubu, gongbu));
            assert_eq!(capability["backends"]["hubu"]["state"], json!(hubu));
            assert_eq!(capability["backends"]["gongbu"]["state"], json!(gongbu));
            assert_eq!(capability["tools"].as_array().unwrap().len(), 33);
            let names = capability["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| tool["name"].as_str().unwrap())
                .collect::<Vec<_>>();
            assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
        }
    }

    #[test]
    fn partial_availability_preserves_unrelated_healthy_tools() {
        let state = snapshot(BackendState::Available, BackendState::Unavailable);
        assert_eq!(
            tool_availability("hubu_list_budgets", BackendOwner::Hubu, &state),
            Ok(())
        );
        assert_eq!(
            tool_availability("gongbu_get_execution", BackendOwner::Gongbu, &state),
            Err(ToolRejection::Unavailable)
        );
    }

    #[test]
    fn degraded_gongbu_keeps_reads_but_blocks_execution_admission() {
        let state = snapshot(BackendState::Available, BackendState::Degraded);
        assert_eq!(
            tool_availability("gongbu_get_artifact", BackendOwner::Gongbu, &state),
            Ok(())
        );
        assert_eq!(
            tool_availability("gongbu_create_execution", BackendOwner::Gongbu, &state),
            Err(ToolRejection::NotReady)
        );
    }

    #[test]
    fn governed_execution_fails_closed_on_required_hubu_state() {
        for hubu in [
            BackendState::Unconfigured,
            BackendState::Unavailable,
            BackendState::Incompatible,
        ] {
            let state = snapshot(hubu, BackendState::Available);
            assert!(
                tool_availability("gongbu_create_execution", BackendOwner::Gongbu, &state).is_err()
            );
            assert_eq!(
                tool_availability("gongbu_get_execution", BackendOwner::Gongbu, &state),
                Ok(())
            );
        }
    }

    #[test]
    fn exact_compatibility_matrix_accepts_matching_backends() {
        let commit = "a".repeat(40);
        let health = probe(200, json!({"status":"ok"}));
        let hubu_version = probe(
            200,
            json!({
                "product_version":"1.2.3",
                "source_commit":commit,
                "executor_contract":EXECUTOR_CONTRACT_VERSION
            }),
        );
        assert_eq!(
            classify_hubu_for(Some(&health), Some(&hubu_version), "1.2.3", &commit).state,
            BackendState::Available
        );

        let live = probe(200, json!({"status":"live"}));
        let ready = probe(200, json!({"status":"ready"}));
        let gongbu_version = probe(
            200,
            json!({
                "product_version":"1.2.3",
                "source_commit":commit,
                "api_schema_version":2,
                "mcp_protocol_version":MCP_PROTOCOL_VERSION,
                "mcp_schema_version":2,
                "hubu_executor_contract":EXECUTOR_CONTRACT_VERSION
            }),
        );
        assert_eq!(
            classify_gongbu_for(
                Some(&live),
                Some(&ready),
                Some(&gongbu_version),
                "1.2.3",
                &commit
            )
            .state,
            BackendState::Available
        );
    }

    #[test]
    fn compatibility_mismatches_and_unknown_commits_fail_closed() {
        let health = probe(200, json!({"status":"ok"}));
        let version = probe(
            200,
            json!({
                "product_version":"1.2.3",
                "source_commit":"unknown",
                "executor_contract":EXECUTOR_CONTRACT_VERSION
            }),
        );
        let report = classify_hubu_for(Some(&health), Some(&version), "1.2.3", &"a".repeat(40));
        assert_eq!(report.state, BackendState::Incompatible);
        assert_eq!(report.reason_code, Some("source_commit_mismatch"));
    }

    #[test]
    fn every_gongbu_compatibility_dimension_fails_closed() {
        let commit = "a".repeat(40);
        let live = probe(200, json!({"status":"live"}));
        let ready = probe(200, json!({"status":"ready"}));
        let base = json!({
            "product_version":"1.2.3",
            "source_commit":commit,
            "api_schema_version":GONGBU_API_SCHEMA_VERSION,
            "mcp_protocol_version":MCP_PROTOCOL_VERSION,
            "mcp_schema_version":GONGBU_MCP_SCHEMA_VERSION,
            "hubu_executor_contract":EXECUTOR_CONTRACT_VERSION
        });
        let cases = [
            (
                "product_version",
                json!("9.9.9"),
                "product_version_mismatch",
            ),
            (
                "source_commit",
                json!("b".repeat(40)),
                "source_commit_mismatch",
            ),
            (
                "hubu_executor_contract",
                json!("hubu-spend-executor-v0"),
                "executor_contract_mismatch",
            ),
            (
                "api_schema_version",
                json!(1),
                "api_schema_version_mismatch",
            ),
            (
                "mcp_schema_version",
                json!(1),
                "mcp_schema_version_mismatch",
            ),
            (
                "mcp_protocol_version",
                json!("2099-01-01"),
                "mcp_protocol_version_mismatch",
            ),
        ];
        for (field, mismatch, expected_reason) in cases {
            let mut body = base.clone();
            body[field] = mismatch;
            let version = probe(200, body);
            let report =
                classify_gongbu_for(Some(&live), Some(&ready), Some(&version), "1.2.3", &commit);
            assert_eq!(report.state, BackendState::Incompatible, "{field}");
            assert_eq!(report.reason_code, Some(expected_reason), "{field}");
        }
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
    fn unavailable_backend_errors_are_actionable_and_redacted() {
        let response = backend_error_response(
            json!(7),
            "hubu_list_budgets",
            BackendOwner::Hubu,
            ToolRejection::Unavailable,
        );
        assert_eq!(response["error"]["code"], -32010);
        assert_eq!(response["error"]["data"]["code"], "backend_unavailable");
        assert_eq!(response["error"]["data"]["retryable"], true);
        assert_eq!(response["error"]["data"]["capabilities_changed"], true);
        let serialized = response.to_string();
        assert!(!serialized.contains("endpoint"));
        assert!(!serialized.contains("credential"));
        assert!(!serialized.contains("token"));
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
