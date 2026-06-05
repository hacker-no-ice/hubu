use std::{
    collections::{BTreeMap, HashMap},
    env,
    fmt::Write as _,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    str::FromStr,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex},
    time::Instant,
};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use hubu_common::{
    ids::{AgentId, BudgetId, SpendAuthTokenId, UserId},
    models::account::{AccountStatus, AgentAccount},
    models::identity::{
        AgentType, CodeReference, ModelIdentity, RuntimeEnvironment, RuntimeIdentity,
    },
    models::{User, UserContext},
    money::Currency,
    time::TimePeriod,
};
use hubu_core::{
    budget::{
        BudgetManager, BudgetManagerError, BudgetRecurrence, BudgetScope, BudgetWithBalance,
        CreateBudgetSeriesRequest, CreateSingleBudgetRequest, ReleaseBudgetResponse,
        ReserveBudgetRequest, ReserveBudgetResponse, SettleBudgetResponse,
    },
    persistence::{
        BudgetRepository, PolicyRepository, SpendRepository, SqliteGovernanceRepository,
    },
    policy::{
        condition::{Condition, Field, PolicyValue},
        engine::validate_policy,
        model::{Effect, Policy, Rule},
    },
    registration::{RegisterAgentRequest, RegistrationManager},
    spend::{
        model::{SpendPaymentValidationRequest, SpendRequest},
        SpendManager,
    },
    telemetry::{configure_file_logging, log_event},
    user::{CreateUserRequest, UserManager},
};
use hubu_wallet::{
    LedgerTransaction, MockPaymentRail, PaymentAttemptRepository, PaymentDestination, PaymentError,
    PaymentManager, PaymentRailKind, PaymentRequest, PaymentStatus, SpendAuthorizationValidator,
    SqliteLedger, SqlitePaymentAttemptRepository,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const DEFAULT_BIND_ADDR: &str = "127.0.0.1:8787";
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

type DemoPaymentManager = PaymentManager<MockPaymentRail, SharedSpendAuthorizer>;

pub fn run_server_from_env() -> Result<()> {
    let bind_addr = env::args()
        .nth(1)
        .or_else(|| env::var("HUBU_BIND_ADDR").ok())
        .unwrap_or_else(|| DEFAULT_BIND_ADDR.to_string());
    run_server(&bind_addr)
}

pub fn run_server(bind_addr: &str) -> Result<()> {
    configure_server_logging()?;
    log_event(
        "info",
        "server_starting",
        json!({
            "bind_addr": bind_addr,
        }),
    );
    let listener = TcpListener::bind(bind_addr)
        .with_context(|| format!("bind Hubu demo server to {bind_addr}"))?;
    let state = ServerState::new()?;

    log_event(
        "info",
        "server_listening",
        json!({
            "bind_addr": bind_addr,
            "url": format!("http://{bind_addr}"),
        }),
    );
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(error) = handle_connection(stream, &state) {
                    log_event(
                        "error",
                        "request_error",
                        json!({
                            "error": error.to_string(),
                        }),
                    );
                }
            }
            Err(error) => log_event(
                "error",
                "connection_error",
                json!({
                    "error": error.to_string(),
                }),
            ),
        }
    }

    Ok(())
}

fn configure_server_logging() -> Result<()> {
    if let Ok(log_path) = env::var("HUBU_LOG_FILE") {
        configure_file_logging(&log_path)
            .with_context(|| format!("configure Hubu log file at {log_path}"))?;
        log_event(
            "info",
            "log_file_configured",
            json!({
                "path": log_path,
            }),
        );
    }
    Ok(())
}

struct ServerState {
    users: Mutex<UserManager>,
    registration: Mutex<RegistrationManager>,
    spend: Arc<Mutex<SpendManager>>,
    budgets: Mutex<BudgetManager>,
    policies: Mutex<HashMap<(UserId, AgentId), Policy>>,
    governance: Mutex<SqliteGovernanceRepository>,
    payment_attempts: Mutex<SqlitePaymentAttemptRepository>,
    payments: Mutex<DemoPaymentManager>,
    image_provider: ImageProviderConfig,
}

#[derive(Debug, Clone)]
struct ImageProviderConfig {
    provider: String,
    model: String,
    merchant: String,
    api_key: Option<String>,
    output_dir: PathBuf,
    adapter_kind: ImageProviderAdapterKind,
}

impl ImageProviderConfig {
    fn from_env() -> Self {
        let provider =
            env::var("HUBU_IMAGE_PROVIDER_NAME").unwrap_or_else(|_| "hubu-demo".to_string());
        Self {
            adapter_kind: image_provider_adapter_kind_from_env(&provider),
            provider,
            model: env::var("HUBU_IMAGE_PROVIDER_MODEL")
                .unwrap_or_else(|_| "demo-image-v1".to_string()),
            merchant: env::var("HUBU_IMAGE_PROXY_MERCHANT")
                .unwrap_or_else(|_| "hubu-model-proxy".to_string()),
            api_key: env::var("HUBU_IMAGE_PROVIDER_API_KEY").ok(),
            output_dir: image_output_dir_from_env(),
        }
    }

    fn resolve(&self, provider: Option<String>, model: Option<String>) -> Result<(String, String)> {
        let provider = provider.unwrap_or_else(|| self.provider.clone());
        let model = model.unwrap_or_else(|| self.model.clone());
        if provider != self.provider || model != self.model {
            return Err(anyhow!(
                "requested image provider/model is not configured in Hubu"
            ));
        }
        Ok((provider, model))
    }

    fn has_api_key(&self) -> bool {
        self.api_key.as_ref().is_some_and(|key| !key.is_empty())
    }

    fn adapter(&self) -> Result<Box<dyn ImageProviderAdapter + '_>> {
        match &self.adapter_kind {
            ImageProviderAdapterKind::Demo => {
                if self.provider != "hubu-demo" {
                    return Err(anyhow!(
                        "demo image adapter can only be used with the hubu-demo provider"
                    ));
                }
                Ok(Box::new(DemoImageProviderAdapter { config: self }))
            }
            ImageProviderAdapterKind::Unsupported(adapter) => Err(anyhow!(
                "image provider adapter '{adapter}' is not supported by this Hubu build"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ImageProviderAdapterKind {
    Demo,
    Unsupported(String),
}

impl ImageProviderAdapterKind {
    fn label(&self) -> &str {
        match self {
            Self::Demo => "demo",
            Self::Unsupported(adapter) => adapter,
        }
    }

    fn is_configured(&self) -> bool {
        matches!(self, Self::Demo)
    }
}

fn image_provider_adapter_kind_from_env(provider: &str) -> ImageProviderAdapterKind {
    match env::var("HUBU_IMAGE_PROVIDER_ADAPTER") {
        Ok(adapter) if adapter == "demo" => ImageProviderAdapterKind::Demo,
        Ok(adapter) => ImageProviderAdapterKind::Unsupported(adapter),
        Err(_) if provider == "hubu-demo" => ImageProviderAdapterKind::Demo,
        Err(_) => ImageProviderAdapterKind::Unsupported("unconfigured".to_string()),
    }
}

fn image_output_dir_from_env() -> PathBuf {
    env::var("HUBU_IMAGE_OUTPUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("target")
                .join("hubu-image-outputs")
        })
}

struct ImageGenerationRequest<'a> {
    provider: &'a str,
    model: &'a str,
    prompt: &'a str,
    artifact_id: &'a str,
}

struct ImageGenerationOutput {
    output_ref: String,
}

trait ImageProviderAdapter {
    fn generate(&self, request: ImageGenerationRequest<'_>) -> Result<ImageGenerationOutput>;
}

struct DemoImageProviderAdapter<'a> {
    config: &'a ImageProviderConfig,
}

impl ImageProviderAdapter for DemoImageProviderAdapter<'_> {
    fn generate(&self, request: ImageGenerationRequest<'_>) -> Result<ImageGenerationOutput> {
        std::fs::create_dir_all(&self.config.output_dir).with_context(|| {
            format!(
                "create image output directory {}",
                self.config.output_dir.display()
            )
        })?;
        let path = self
            .config
            .output_dir
            .join(format!("hubu-logo-{}.svg", request.artifact_id));
        std::fs::write(
            &path,
            demo_image_svg(request.provider, request.model, request.prompt),
        )
        .with_context(|| format!("write demo image artifact to {}", path.display()))?;
        let absolute_path = if path.is_absolute() {
            path
        } else {
            env::current_dir()?.join(path)
        };
        Ok(ImageGenerationOutput {
            output_ref: format!("file://{}", absolute_path.display()),
        })
    }
}

impl ServerState {
    fn new() -> Result<Self> {
        Self::new_with_db_path(
            env::var("HUBU_DB_PATH").unwrap_or_else(|_| "hubu.sqlite3".to_string()),
        )
    }

    fn new_with_db_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let db_path = path.display().to_string();
        log_event(
            "info",
            "server_state_initializing",
            json!({
                "db_path": db_path,
            }),
        );
        let mut users = UserManager::open(&path).context("initialize user store")?;
        let default_user = users.ensure_default_user()?;
        let mut governance =
            SqliteGovernanceRepository::open(&path).context("initialize governance store")?;
        governance
            .expire_overdue_budget_holds(Utc::now())
            .context("reconcile expired budget holds")?;
        let policy_assignments = governance
            .load_policy_assignments()
            .context("load policy assignments")?;
        let policies = policy_assignments
            .into_iter()
            .map(|assignment| {
                (
                    (assignment.owner_user_id, assignment.agent_id),
                    assignment.policy,
                )
            })
            .collect();
        let spend = Arc::new(Mutex::new(SpendManager::from_records(
            governance
                .load_spend_decisions()
                .context("load spend decisions")?,
            governance
                .load_spend_auth_tokens()
                .context("load spend auth tokens")?,
        )));
        let budgets = BudgetManager::from_records(
            governance.load_budgets().context("load budgets")?,
            governance
                .load_budget_balances()
                .context("load budget balances")?,
            governance
                .load_budget_holds()
                .context("load budget holds")?,
        );
        let authorizer = SharedSpendAuthorizer {
            spend: Arc::clone(&spend),
        };
        let mut payments = PaymentManager::new(
            default_user.id.clone(),
            MockPaymentRail,
            authorizer,
            SqliteLedger::open(&path).context("initialize ledger")?,
        )
        .context("initialize payment manager")?;
        let payment_attempts = SqlitePaymentAttemptRepository::open(&path)
            .context("initialize payment attempt store")?;
        for attempt in payment_attempts
            .list_payment_attempts()
            .context("load payment attempts")?
        {
            payments
                .remember_payment_attempt(attempt.request(), attempt.response())
                .context("hydrate payment idempotency")?;
        }

        let state = Self {
            users: Mutex::new(users),
            registration: Mutex::new(
                RegistrationManager::open(&path).context("initialize agent registration store")?,
            ),
            spend,
            budgets: Mutex::new(budgets),
            policies: Mutex::new(policies),
            governance: Mutex::new(governance),
            payment_attempts: Mutex::new(payment_attempts),
            payments: Mutex::new(payments),
            image_provider: ImageProviderConfig::from_env(),
        };
        log_event(
            "info",
            "server_state_initialized",
            json!({
                "db_path": db_path,
                "default_user_id": default_user.id.to_string(),
                "default_user_pub_id": default_user.pub_id,
                "image_provider": state.image_provider.provider.clone(),
                "image_model": state.image_provider.model.clone(),
                "image_proxy_merchant": state.image_provider.merchant.clone(),
                "image_provider_api_key_configured": state.image_provider.has_api_key(),
                "image_provider_adapter": state.image_provider.adapter_kind.label(),
                "image_provider_adapter_configured": state.image_provider.adapter_kind.is_configured(),
                "image_output_dir": state.image_provider.output_dir.display().to_string(),
            }),
        );
        Ok(state)
    }
}

#[derive(Clone)]
struct SharedSpendAuthorizer {
    spend: Arc<Mutex<SpendManager>>,
}

impl SpendAuthorizationValidator for SharedSpendAuthorizer {
    fn validate_payment_request(
        &self,
        request: &PaymentRequest,
    ) -> Result<hubu_wallet::ValidatedSpendAuthorization, PaymentError> {
        self.spend
            .lock()
            .map_err(|_| PaymentError::AuthorizationRejected {
                reason: "spend manager lock poisoned".to_string(),
            })?
            .validate_auth_token_for_payment(&SpendPaymentValidationRequest {
                spend_auth_token_id: request.spend_auth_token_id.clone(),
                owner_user_id: request.owner_user_id.clone(),
                agent_id: request.agent_id.clone(),
                agent_account_id: request.agent_account_id.clone(),
                amount_cents: request.amount_cents,
                currency: request.currency,
                merchant: request.merchant.clone(),
                task_id: request.task_id.clone(),
            })
            .map(|validation| hubu_wallet::ValidatedSpendAuthorization {
                spend_auth_token_id: request.spend_auth_token_id.clone(),
                owner_user_id: validation.owner_user_id,
            })
            .map_err(|error| PaymentError::AuthorizationRejected {
                reason: error.to_string(),
            })
    }

    fn mark_token_used(
        &mut self,
        token_id: &SpendAuthTokenId,
        payment_id: &hubu_common::ids::PaymentId,
    ) -> Result<(), PaymentError> {
        self.spend
            .lock()
            .map_err(|_| PaymentError::AuthorizationRejected {
                reason: "spend manager lock poisoned".to_string(),
            })?
            .mark_auth_token_used(token_id, payment_id.clone())
            .map_err(|error| PaymentError::AuthorizationRejected {
                reason: error.to_string(),
            })
    }
}

const REGISTRATION_PROTOCOL_VERSION: &str = "hubu-agent-registration-v1";
const FINGERPRINT_PREFIX: &str = "sha256:";

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum RegisterAgentHttpRequest {
    Envelope(RegistrationEnvelope),
    Simple(SimpleRegisterAgentHttpRequest),
}

#[derive(Debug, Serialize, Deserialize)]
struct SimpleRegisterAgentHttpRequest {
    name: String,
    version: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct RegistrationEnvelope {
    protocol_version: String,
    identity: RegistrationPayloadWithFingerprint,
    version: RegistrationPayloadWithFingerprint,
    review: Option<RegistrationReview>,
    signature: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RegistrationPayloadWithFingerprint {
    payload: Value,
    fingerprint: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct RegistrationReview {
    display_name: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Serialize)]
struct RegisterAgentHttpResponse {
    user_id: String,
    agent_id: String,
    agent_pub_id: String,
    version_id: String,
    account_id: String,
    session_id: String,
}

#[derive(Debug, Serialize)]
struct CurrentUserHttpResponse {
    user_id: String,
    display_name: String,
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InitHttpRequest {
    display_name: Option<String>,
    email: Option<String>,
}

#[derive(Debug, Serialize)]
struct InitHttpResponse {
    user_id: String,
    display_name: String,
}

#[derive(Debug, Deserialize)]
struct AddPolicyHttpRequest {
    agent_id: String,
    daily_limit_cents: Option<i64>,
    policy_yaml: Option<String>,
}

#[derive(Debug, Serialize)]
struct AddPolicyHttpResponse {
    agent_id: String,
    policy_id: String,
    policy_version: String,
    default_decision: String,
}

#[derive(Debug, Serialize)]
struct AgentListHttpResponse {
    agents: Vec<AgentHttpResponse>,
}

#[derive(Debug, Serialize)]
struct AgentHttpResponse {
    agent_id: String,
    display_name: String,
    description: Option<String>,
    agent_type: String,
    status: String,
    account_id: String,
    account_status: String,
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct CreateBudgetHttpRequest {
    agent_id: Option<String>,
    amount_cents: i64,
    starting_at: Option<String>,
    ending_before: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateBudgetSeriesHttpRequest {
    amount_cents: i64,
    starting_at: Option<String>,
    recurrence: BudgetRecurrenceHttp,
    period_count: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BudgetRecurrenceHttp {
    Daily,
    Monthly,
    Yearly,
}

#[derive(Debug, Serialize)]
struct CreateBudgetHttpResponse {
    budget: BudgetHttpResponse,
}

#[derive(Debug, Serialize)]
struct CreateBudgetSeriesHttpResponse {
    budgets: Vec<BudgetHttpResponse>,
}

#[derive(Debug, Serialize)]
struct ListBudgetsHttpResponse {
    budgets: Vec<BudgetHttpResponse>,
}

#[derive(Debug, Serialize)]
struct BudgetHttpResponse {
    budget_id: String,
    scope: String,
    amount_limit_cents: i64,
    currency: String,
    starting_at: String,
    ending_before: Option<String>,
    status: String,
    consumed_amount_cents: i64,
    frozen_amount_cents: i64,
    remaining_amount_cents: i64,
}

#[derive(Debug, Deserialize)]
struct SpendHttpRequest {
    agent_id: Option<String>,
    account_id: Option<String>,
    amount_cents: i64,
    reason: String,
    merchant: Option<String>,
    budget_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct SpendHttpResponse {
    account_id: String,
    agent_id: String,
    decision_id: String,
    decision: String,
    reasons: Vec<String>,
    auth_token_id: Option<String>,
    budget_hold: Option<BudgetHoldHttpResponse>,
    payment: Option<PaymentHttpResponse>,
}

#[derive(Debug, Deserialize)]
struct GenerateImageHttpRequest {
    spend_auth_token_id: String,
    prompt: String,
    provider: Option<String>,
    model: Option<String>,
}

#[derive(Debug, Serialize)]
struct GenerateImageHttpResponse {
    provider: String,
    model: String,
    output_ref: String,
    spend_auth_token_id: String,
    payment: PaymentHttpResponse,
    budget_hold: BudgetHoldHttpResponse,
}

#[derive(Debug, Serialize)]
struct BudgetHoldHttpResponse {
    hold_id: String,
    budget_id: String,
    status: String,
    amount_cents: i64,
    consumed_amount_cents: i64,
    frozen_amount_cents: i64,
    remaining_amount_cents: i64,
}

#[derive(Debug, Serialize)]
struct PaymentHttpResponse {
    payment_id: String,
    owner_user_id: String,
    owner_user_name: String,
    account_id: String,
    status: String,
    ledger_transaction_id: Option<String>,
    rail_reference: Option<String>,
    failure_reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct LedgerHttpResponse {
    transactions: Vec<LedgerTransactionHttpResponse>,
}

#[derive(Debug, Serialize)]
struct LedgerTransactionHttpResponse {
    id: String,
    owner_user_id: String,
    owner_user_name: String,
    external_ref: Option<String>,
    description: String,
    created_at: String,
    entries: Vec<LedgerEntryHttpResponse>,
}

#[derive(Debug, Serialize)]
struct LedgerEntryHttpResponse {
    id: String,
    owner_user_id: String,
    owner_user_name: String,
    account_id: String,
    direction: String,
    amount_cents: i64,
    currency: String,
}

fn handle_connection(mut stream: TcpStream, state: &ServerState) -> Result<()> {
    let started_at = Instant::now();
    let request_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let mut raw = String::new();
    stream.read_to_string(&mut raw)?;
    if raw.is_empty() {
        log_event(
            "debug",
            "http_empty_request",
            json!({
                "request_id": request_id,
            }),
        );
        return Ok(());
    }

    let request = parse_request(&raw)?;
    log_event(
        "info",
        "http_request_started",
        json!({
            "request_id": request_id,
            "method": request.method,
            "path": request.path,
            "body_bytes": request.body.len(),
        }),
    );
    let method = request.method.clone();
    let path = request.path.clone();
    let response = route(request, state);
    let status = response.status;
    write_response(&mut stream, response)
        .with_context(|| format!("write response for request {request_id}"))?;
    log_event(
        "info",
        "http_request_finished",
        json!({
            "request_id": request_id,
            "method": method,
            "path": path,
            "status": status,
            "elapsed_ms": started_at.elapsed().as_millis(),
        }),
    );
    Ok(())
}

fn route(request: HttpRequest, state: &ServerState) -> HttpResponse {
    let result = match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") => Ok(json!({ "status": "ok" })),
        ("GET", "/registration/guidance")
        | ("GET", "/.well-known/hubu-agent-registration.json") => Ok(registration_guidance()),
        ("GET", "/user") => current_user(state).map(to_json),
        ("POST", "/init") => init(request.body, state).map(to_json),
        ("POST", "/agents/register") => register_agent(request.body, state).map(to_json),
        ("GET", "/agents") => list_agents(state).map(to_json),
        ("POST", "/policies") => add_policy(request.body, state).map(to_json),
        ("POST", "/budgets") => create_budget(request.body, state).map(to_json),
        ("POST", "/budgets/series") => create_budget_series(request.body, state).map(to_json),
        ("GET", "/budgets") => list_budgets(state).map(to_json),
        ("POST", "/spend/authorize") => authorize_spend(request.body, state).map(to_json),
        ("POST", "/spend") => spend(request.body, state).map(to_json),
        ("POST", "/model-calls/image") => generate_image(request.body, state).map(to_json),
        ("GET", "/ledger") => list_ledger(state).map(to_json),
        _ => Err(anyhow!("no route for {} {}", request.method, request.path)),
    };

    match result {
        Ok(body) => HttpResponse { status: 200, body },
        Err(error) => {
            log_event(
                "warn",
                "http_request_rejected",
                json!({
                    "method": request.method,
                    "path": request.path,
                    "error": error.to_string(),
                }),
            );
            HttpResponse {
                status: 400,
                body: json!({ "error": error.to_string() }),
            }
        }
    }
}

fn registration_guidance() -> Value {
    json!({
        "protocol_version": "hubu-agent-registration-v1",
        "fingerprint": {
            "algorithm": "sha256",
            "encoding": "hex",
            "prefix": "sha256:",
            "canonicalization": "canonical_json_v1"
        },
        "signature_policy": "not_supported",
        "human_inputs": [
            {
                "name": "agent_name",
                "required": true,
                "prompt": "Agent name",
                "default_strategy": "workspace_or_runtime_name"
            },
            {
                "name": "version_label",
                "required": true,
                "prompt": "Version",
                "default_strategy": "git_commit_or_dev"
            }
        ],
        "client_filled": {
            "agent_identity.vendor": "codex",
            "agent_name.default_template": "{vendor}-{workspace}",
            "owner": "active_hubu_user",
            "agent_kind": "codex_agent",
            "runtime.provider": "codex",
            "runtime.environment": "development",
            "hubu_client.name": "current_client_name",
            "hubu_client.version": "current_client_version"
        },
        "identity_payload": {
            "required": [
                "protocol_version",
                "owner",
                "agent_name",
                "agent_kind"
            ],
            "optional": [
                "source_repository_url",
                "package_ref",
                "issuer",
                "agent_public_key_id"
            ]
        },
        "version_payload": {
            "required": [
                "protocol_version",
                "identity_fingerprint",
                "version_label",
                "runtime",
                "hubu_client"
            ],
            "optional": [
                "code",
                "model",
                "tool_manifest_digest",
                "permission_manifest_digest",
                "config_digest"
            ]
        },
        "review_fields": [
            "agent_name",
            "owner",
            "agent_kind",
            "version_label",
            "runtime.provider",
            "runtime.environment",
            "code.repository_url"
        ]
    })
}

fn init(body: String, state: &ServerState) -> Result<InitHttpResponse> {
    let request: InitHttpRequest = if body.trim().is_empty() {
        InitHttpRequest {
            display_name: None,
            email: None,
        }
    } else {
        serde_json::from_str(&body)?
    };

    let user = state
        .users
        .lock()
        .map_err(|_| anyhow!("user manager lock poisoned"))?
        .create_user(CreateUserRequest {
            display_name: request
                .display_name
                .unwrap_or_else(|| "Hubu User".to_string()),
            email: request.email,
        })?;

    Ok(InitHttpResponse {
        user_id: user.pub_id,
        display_name: user.display_name,
    })
}

fn current_user(state: &ServerState) -> Result<CurrentUserHttpResponse> {
    let user = default_user(state)?;
    Ok(CurrentUserHttpResponse {
        user_id: user.pub_id,
        display_name: user.display_name,
        email: user.email,
    })
}

fn register_agent(body: String, state: &ServerState) -> Result<RegisterAgentHttpResponse> {
    let request: RegisterAgentHttpRequest = serde_json::from_str(&body)?;
    let user = default_user(state)?;
    let (request_shape, envelope) = match request {
        RegisterAgentHttpRequest::Envelope(envelope) => ("envelope", envelope),
        RegisterAgentHttpRequest::Simple(request) => (
            "simple",
            simple_registration_envelope(&request.name, &request.version, &user.pub_id),
        ),
    };
    log_event(
        "info",
        "agent_registration_received",
        json!({
            "request_shape": request_shape,
            "user_id": user.id.to_string(),
            "user_pub_id": user.pub_id,
            "protocol_version": envelope.protocol_version,
        }),
    );
    let registration_request = registration_request_from_envelope(envelope, &user)?;

    let response = state
        .registration
        .lock()
        .map_err(|_| anyhow!("registration manager lock poisoned"))?
        .register_agent(registration_request)?;

    log_event(
        "info",
        "agent_registration_completed",
        json!({
            "user_id": user.id.to_string(),
            "user_pub_id": user.pub_id,
            "agent_id": response.agent.id.to_string(),
            "agent_pub_id": response.agent.pub_id,
            "version_id": response.version.id.to_string(),
            "version_pub_id": response.version.pub_id,
            "account_id": response.account.id.to_string(),
            "account_pub_id": response.account.pub_id,
            "session_id": response.session.id.to_string(),
            "session_pub_id": response.session.pub_id,
        }),
    );
    Ok(RegisterAgentHttpResponse {
        user_id: user.pub_id,
        agent_id: response.agent.pub_id.clone(),
        agent_pub_id: response.agent.pub_id,
        version_id: response.version.pub_id,
        account_id: response.account.pub_id,
        session_id: response.session.pub_id,
    })
}

fn simple_registration_envelope(
    agent_name: &str,
    version_label: &str,
    owner_pub_id: &str,
) -> RegistrationEnvelope {
    let identity_payload = json!({
        "protocol_version": REGISTRATION_PROTOCOL_VERSION,
        "owner": {
            "type": "hubu_user",
            "pub_id": owner_pub_id
        },
        "agent_name": agent_name,
        "agent_kind": "codex_agent"
    });
    let identity_fingerprint = fingerprint_payload(&identity_payload);
    let version_payload = json!({
        "protocol_version": REGISTRATION_PROTOCOL_VERSION,
        "identity_fingerprint": identity_fingerprint,
        "version_label": version_label,
        "runtime": {
            "provider": "codex",
            "environment": "development"
        },
        "hubu_client": {
            "name": "hubu-cli",
            "version": env!("CARGO_PKG_VERSION")
        }
    });
    let version_fingerprint = fingerprint_payload(&version_payload);

    RegistrationEnvelope {
        protocol_version: REGISTRATION_PROTOCOL_VERSION.to_string(),
        identity: RegistrationPayloadWithFingerprint {
            payload: identity_payload,
            fingerprint: identity_fingerprint,
        },
        version: RegistrationPayloadWithFingerprint {
            payload: version_payload,
            fingerprint: version_fingerprint,
        },
        review: Some(RegistrationReview {
            display_name: Some(agent_name.to_string()),
            description: Some("Registered through the Hubu demo CLI".to_string()),
        }),
        signature: None,
    }
}

fn registration_request_from_envelope(
    envelope: RegistrationEnvelope,
    user: &User,
) -> Result<RegisterAgentRequest> {
    if envelope.protocol_version != REGISTRATION_PROTOCOL_VERSION {
        return Err(anyhow!(
            "unsupported registration protocol version `{}`",
            envelope.protocol_version
        ));
    }
    if envelope.signature.is_some() {
        return Err(anyhow!("registration signatures are not supported yet"));
    }

    verify_payload_fingerprint("identity", &envelope.identity)?;
    verify_payload_fingerprint("version", &envelope.version)?;
    log_event(
        "info",
        "agent_registration_fingerprints_verified",
        json!({
            "user_id": user.id.to_string(),
            "user_pub_id": user.pub_id,
            "identity_fingerprint": envelope.identity.fingerprint,
            "version_fingerprint": envelope.version.fingerprint,
        }),
    );
    validate_required_registration_payloads(&envelope)?;

    let identity_fingerprint = envelope.identity.fingerprint;
    let version_fingerprint = envelope.version.fingerprint;
    if string_field(&envelope.version.payload, "identity_fingerprint")? != identity_fingerprint {
        return Err(anyhow!(
            "version payload identity_fingerprint does not match identity fingerprint"
        ));
    }

    let owner = envelope
        .identity
        .payload
        .get("owner")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("identity payload missing owner object"))?;
    if owner.get("type").and_then(Value::as_str) != Some("hubu_user") {
        return Err(anyhow!("identity payload owner.type must be `hubu_user`"));
    }
    if owner.get("pub_id").and_then(Value::as_str) != Some(user.pub_id.as_str()) {
        return Err(anyhow!(
            "identity payload owner.pub_id does not match active Hubu user"
        ));
    }

    let agent_name = string_field(&envelope.identity.payload, "agent_name")?;
    let agent_kind = string_field(&envelope.identity.payload, "agent_kind")?;
    let agent_type = match agent_kind.as_str() {
        "codex_agent" => AgentType::AutonomousAgent,
        other => return Err(anyhow!("unsupported agent_kind `{other}`")),
    };
    let version_label = string_field(&envelope.version.payload, "version_label")?;

    let review = envelope.review;
    let display_name = review
        .as_ref()
        .and_then(|review| review.display_name.clone())
        .unwrap_or_else(|| agent_name.clone());
    let description = review
        .and_then(|review| review.description)
        .or_else(|| Some("Registered through the Hubu registration protocol".to_string()));

    Ok(RegisterAgentRequest {
        display_name,
        description,
        owner_user_id: user.id.clone(),
        agent_type,
        identity_fingerprint,
        version_fingerprint,
        code_ref: code_reference_from_payload(&envelope.version.payload),
        model: model_identity_from_payload(&envelope.version.payload),
        runtime: runtime_identity_from_payload(&envelope.version.payload)?,
        mcp_client_name: nested_string_field(&envelope.version.payload, "hubu_client", "name"),
        mcp_client_version: nested_string_field(
            &envelope.version.payload,
            "hubu_client",
            "version",
        )
        .or(Some(version_label)),
    })
}

fn validate_required_registration_payloads(envelope: &RegistrationEnvelope) -> Result<()> {
    require_payload_protocol_version("identity", &envelope.identity.payload)?;
    require_payload_protocol_version("version", &envelope.version.payload)?;
    require_object_field("identity", &envelope.identity.payload, "owner")?;
    string_field(&envelope.identity.payload, "agent_name")?;
    string_field(&envelope.identity.payload, "agent_kind")?;
    string_field(&envelope.version.payload, "identity_fingerprint")?;
    string_field(&envelope.version.payload, "version_label")?;
    let runtime = require_object_field("version", &envelope.version.payload, "runtime")?;
    require_nested_string_field("version", runtime, "runtime", "provider")?;
    require_nested_string_field("version", runtime, "runtime", "environment")?;
    let hubu_client = require_object_field("version", &envelope.version.payload, "hubu_client")?;
    require_nested_string_field("version", hubu_client, "hubu_client", "name")?;
    require_nested_string_field("version", hubu_client, "hubu_client", "version")?;
    Ok(())
}

fn require_payload_protocol_version(label: &str, payload: &Value) -> Result<()> {
    let protocol_version = string_field(payload, "protocol_version")?;
    if protocol_version != REGISTRATION_PROTOCOL_VERSION {
        return Err(anyhow!(
            "{label} payload protocol_version must be `{REGISTRATION_PROTOCOL_VERSION}`"
        ));
    }
    Ok(())
}

fn require_object_field<'a>(
    label: &str,
    payload: &'a Value,
    field: &str,
) -> Result<&'a serde_json::Map<String, Value>> {
    payload
        .get(field)
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("{label} payload missing object field `{field}`"))
}

fn require_nested_string_field(
    label: &str,
    object: &serde_json::Map<String, Value>,
    object_name: &str,
    field: &str,
) -> Result<String> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("{label} payload missing string field `{object_name}.{field}`"))
}

fn verify_payload_fingerprint(
    label: &str,
    payload: &RegistrationPayloadWithFingerprint,
) -> Result<()> {
    let expected = fingerprint_payload(&payload.payload);
    if payload.fingerprint != expected {
        return Err(anyhow!(
            "{label} fingerprint mismatch: expected {expected}, got {}",
            payload.fingerprint
        ));
    }
    Ok(())
}

fn string_field(payload: &Value, field: &str) -> Result<String> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("payload missing string field `{field}`"))
}

fn nested_string_field(payload: &Value, object: &str, field: &str) -> Option<String> {
    payload
        .get(object)
        .and_then(Value::as_object)
        .and_then(|object| object.get(field))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn code_reference_from_payload(payload: &Value) -> Option<CodeReference> {
    let code = payload.get("code")?.as_object()?;
    Some(CodeReference {
        repository_url: code
            .get("repository_url")
            .and_then(Value::as_str)
            .map(str::to_string),
        commit_sha: code
            .get("commit_sha")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn model_identity_from_payload(payload: &Value) -> Option<ModelIdentity> {
    let model = payload.get("model")?.as_object()?;
    Some(ModelIdentity {
        provider: model.get("provider")?.as_str()?.to_string(),
        model: model.get("model")?.as_str()?.to_string(),
        version: model
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn runtime_identity_from_payload(payload: &Value) -> Result<Option<RuntimeIdentity>> {
    let Some(runtime) = payload.get("runtime").and_then(Value::as_object) else {
        return Ok(None);
    };
    let runtime_provider = runtime
        .get("provider")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("version payload runtime.provider must be a string"))?
        .to_string();
    let environment = match runtime
        .get("environment")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("version payload runtime.environment must be a string"))?
    {
        "production" => RuntimeEnvironment::Production,
        "staging" => RuntimeEnvironment::Staging,
        "development" => RuntimeEnvironment::Development,
        other => return Err(anyhow!("unsupported runtime.environment `{other}`")),
    };

    Ok(Some(RuntimeIdentity {
        runtime_provider,
        environment,
    }))
}

fn fingerprint_payload(payload: &Value) -> String {
    let canonical = canonical_json(payload);
    let digest = Sha256::digest(canonical.as_bytes());
    format!("{FINGERPRINT_PREFIX}{}", hex_encode(&digest))
}

fn canonical_json(value: &Value) -> String {
    serde_json::to_string(&canonical_value(value)).expect("canonical JSON should serialize")
}

fn canonical_value(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonical_value).collect()),
        Value::Object(object) => {
            let sorted = object
                .iter()
                .map(|(key, value)| (key.clone(), canonical_value(value)))
                .collect::<BTreeMap<_, _>>();
            Value::Object(sorted.into_iter().collect())
        }
        _ => value.clone(),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to string should not fail");
    }
    encoded
}

fn add_policy(body: String, state: &ServerState) -> Result<AddPolicyHttpResponse> {
    let request: AddPolicyHttpRequest = serde_json::from_str(&body)?;

    let agent_pub_id = request.agent_id;
    let user = default_user_context(state)?;
    let agent_id = resolve_agent_id_for_user(&agent_pub_id, &user, state)?;
    let policy = if let Some(policy_yaml) = request.policy_yaml {
        let mut policy: Policy = serde_yaml::from_str(&policy_yaml)?;
        policy.owner_user_id = user.user_id.clone();
        validate_policy(&policy)?;
        policy
    } else {
        let daily_limit_cents = request
            .daily_limit_cents
            .ok_or_else(|| anyhow!("policy add requires `policy_yaml` or `daily_limit_cents`"))?;
        if daily_limit_cents <= 0 {
            return Err(anyhow!("daily limit must be positive"));
        }

        Policy {
            id: format!("demo_policy_{agent_pub_id}"),
            version: "demo-1".to_string(),
            owner_user_id: user.user_id.clone(),
            default_effect: Effect::NeedsApproval,
            rules: vec![
                Rule {
                    id: "deny_blocked_demo_merchant".to_string(),
                    effect: Effect::Deny,
                    when: Condition::Eq {
                        field: Field::Merchant,
                        value: PolicyValue::String("blocked-merchant".to_string()),
                    },
                    reason: "merchant is blocked by the demo policy".to_string(),
                },
                Rule {
                    id: "allow_within_demo_limit".to_string(),
                    effect: Effect::Allow,
                    when: Condition::Lte {
                        field: Field::Amount,
                        value: PolicyValue::MoneyCents(daily_limit_cents),
                    },
                    reason: format!(
                        "amount is at or below the configured demo limit of {} cents",
                        daily_limit_cents
                    ),
                },
            ],
        }
    };

    let policy_id = policy.id.clone();
    let policy_version = policy.version.clone();
    let default_decision = effect_name(policy.default_effect).to_string();
    state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?
        .save_policy_assignment(&user.user_id, &agent_id, &policy)?;

    state
        .policies
        .lock()
        .map_err(|_| anyhow!("policy store lock poisoned"))?
        .insert((user.user_id, agent_id), policy);

    log_event(
        "info",
        "policy_added",
        json!({
            "agent_pub_id": agent_pub_id,
            "policy_id": policy_id,
            "policy_version": policy_version,
            "default_decision": default_decision,
        }),
    );
    Ok(AddPolicyHttpResponse {
        agent_id: agent_pub_id,
        policy_id,
        policy_version,
        default_decision,
    })
}

fn list_agents(state: &ServerState) -> Result<AgentListHttpResponse> {
    let user = default_user_context(state)?;
    let agents = state
        .registration
        .lock()
        .map_err(|_| anyhow!("registration manager lock poisoned"))?
        .agents_for_user(&user.user_id)?
        .into_iter()
        .map(|agent| AgentHttpResponse {
            agent_id: agent.agent.pub_id,
            display_name: agent.agent.display_name,
            description: agent.agent.description,
            agent_type: match agent.agent.agent_type {
                AgentType::InteractiveAgent => "interactive_agent",
                AgentType::AutonomousAgent => "agent",
            }
            .to_string(),
            status: match agent.agent.agent_status {
                hubu_common::models::identity::AgentStatus::Active => "active",
                hubu_common::models::identity::AgentStatus::Suspended => "suspended",
            }
            .to_string(),
            account_id: agent.account.pub_id,
            account_status: match agent.account.account_status {
                hubu_common::models::account::AccountStatus::Active => "active",
                hubu_common::models::account::AccountStatus::Suspended => "suspended",
            }
            .to_string(),
            created_at: agent.agent.created_at.to_rfc3339(),
        })
        .collect();

    Ok(AgentListHttpResponse { agents })
}

fn create_budget(body: String, state: &ServerState) -> Result<CreateBudgetHttpResponse> {
    let request: CreateBudgetHttpRequest = serde_json::from_str(&body)?;
    if request.amount_cents <= 0 {
        return Err(anyhow!("budget amount must be positive"));
    }

    let user = default_user_context(state)?;
    let scope = budget_scope_from_create_request(&request, &user, state)?;
    let period = TimePeriod::new(
        parse_optional_datetime(request.starting_at)?.unwrap_or_else(Utc::now),
        parse_optional_datetime(request.ending_before)?,
    )
    .map_err(|error| anyhow!("invalid budget period: {error:?}"))?;

    let response = state
        .budgets
        .lock()
        .map_err(|_| anyhow!("budget manager lock poisoned"))?
        .create_single_budget(CreateSingleBudgetRequest {
            scope,
            amount_limit_cents: request.amount_cents,
            currency: Currency::Usd,
            period,
        })?;
    state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?
        .save_budget_with_balance(&response.budget, &response.balance)?;

    log_event(
        "info",
        "budget_created",
        json!({
            "budget_id": response.budget.id.to_string(),
            "user_id": user.user_id.to_string(),
            "amount_cents": response.budget.amount_limit_cents,
            "currency": response.budget.currency.to_string(),
            "starting_at": response.budget.period.starting_at.to_rfc3339(),
            "ending_before": response.budget.period.ending_before.map(|value| value.to_rfc3339()),
        }),
    );
    Ok(CreateBudgetHttpResponse {
        budget: budget_response(BudgetWithBalance {
            budget: response.budget,
            balance: response.balance,
        }),
    })
}

fn budget_scope_from_create_request(
    request: &CreateBudgetHttpRequest,
    user: &UserContext,
    state: &ServerState,
) -> Result<BudgetScope> {
    let Some(agent_pub_id) = request.agent_id.as_deref() else {
        return Ok(BudgetScope::User(user.user_id.clone()));
    };
    let registration = state
        .registration
        .lock()
        .map_err(|_| anyhow!("registration manager lock poisoned"))?;
    let agent_id = registration
        .agent_id_for_pub_id(agent_pub_id)?
        .ok_or_else(|| anyhow!("unknown public agent id {agent_pub_id}"))?;
    let account = registration
        .account_for_agent(&agent_id)?
        .ok_or_else(|| anyhow!("no account found for agent {agent_pub_id}"))?;
    if account.owner_user_id != user.user_id {
        return Err(anyhow!(
            "agent {agent_pub_id} is not owned by resolved user"
        ));
    }
    Ok(BudgetScope::Agent(agent_id))
}

fn create_budget_series(
    body: String,
    state: &ServerState,
) -> Result<CreateBudgetSeriesHttpResponse> {
    let request: CreateBudgetSeriesHttpRequest = serde_json::from_str(&body)?;
    if request.amount_cents <= 0 {
        return Err(anyhow!("budget amount must be positive"));
    }

    let user = default_user_context(state)?;
    let starting_at = parse_optional_datetime(request.starting_at)?.unwrap_or_else(Utc::now);

    let response = state
        .budgets
        .lock()
        .map_err(|_| anyhow!("budget manager lock poisoned"))?
        .create_budget_series(CreateBudgetSeriesRequest {
            scope: BudgetScope::User(user.user_id.clone()),
            amount_limit_cents: request.amount_cents,
            currency: Currency::Usd,
            starting_at,
            recurrence: match request.recurrence {
                BudgetRecurrenceHttp::Daily => BudgetRecurrence::Daily,
                BudgetRecurrenceHttp::Monthly => BudgetRecurrence::Monthly,
                BudgetRecurrenceHttp::Yearly => BudgetRecurrence::Yearly,
            },
            period_count: request.period_count,
        })?;
    {
        let mut governance = state
            .governance
            .lock()
            .map_err(|_| anyhow!("governance store lock poisoned"))?;
        for budget in &response.budgets {
            governance.save_budget_with_balance(&budget.budget, &budget.balance)?;
        }
    }

    log_event(
        "info",
        "budget_series_created",
        json!({
            "user_id": user.user_id.to_string(),
            "budget_count": response.budgets.len(),
        }),
    );
    Ok(CreateBudgetSeriesHttpResponse {
        budgets: response.budgets.into_iter().map(budget_response).collect(),
    })
}

fn list_budgets(state: &ServerState) -> Result<ListBudgetsHttpResponse> {
    let user = default_user_context(state)?;
    reconcile_expired_budget_holds(state)?;
    let agent_ids = state
        .registration
        .lock()
        .map_err(|_| anyhow!("registration manager lock poisoned"))?
        .agents_for_user(&user.user_id)?
        .into_iter()
        .map(|agent| agent.agent.id)
        .collect::<Vec<_>>();
    let budgets = {
        let budgets = state
            .budgets
            .lock()
            .map_err(|_| anyhow!("budget manager lock poisoned"))?;
        let mut visible_budgets = budgets.get_budgets_by_user_id(&user.user_id);
        for agent_id in agent_ids {
            visible_budgets.extend(budgets.get_budgets_by_agent_id(&agent_id));
        }
        visible_budgets.into_iter().map(budget_response).collect()
    };

    Ok(ListBudgetsHttpResponse { budgets })
}

fn authorize_spend(body: String, state: &ServerState) -> Result<SpendHttpResponse> {
    let request: SpendHttpRequest = serde_json::from_str(&body)?;
    let authorization = match evaluate_and_reserve_spend(request, state)? {
        SpendAuthorization::Authorized(authorization) => authorization,
        SpendAuthorization::Response(response) => return Ok(response),
    };

    Ok(SpendHttpResponse {
        account_id: authorization.account_pub_id,
        agent_id: authorization.agent_pub_id,
        decision_id: authorization.evaluation.decision_id.to_string(),
        decision: effect_name(authorization.evaluation.evaluation.decision).to_string(),
        reasons: authorization.evaluation.evaluation.reasons,
        auth_token_id: authorization.auth_token_id,
        budget_hold: authorization.reservation.map(frozen_budget_hold_response),
        payment: None,
    })
}

fn spend(body: String, state: &ServerState) -> Result<SpendHttpResponse> {
    let request: SpendHttpRequest = serde_json::from_str(&body)?;
    let authorization = match evaluate_and_reserve_spend(request, state)? {
        SpendAuthorization::Authorized(authorization) => authorization,
        SpendAuthorization::Response(response) => return Ok(response),
    };
    let owner = owner_metadata_for_user_id(&authorization.user.user_id, state)?;
    let (budget_hold, payment) = if let Some(token) = authorization.token {
        let reservation = authorization
            .reservation
            .ok_or_else(|| anyhow!("allowed spend did not reserve budget"))?;
        let payment_request = PaymentRequest {
            idempotency_key: format!(
                "{}:{}",
                authorization.evaluation.decision_id, authorization.reason
            ),
            spend_auth_token_id: token.id,
            owner_user_id: authorization.user.user_id.clone(),
            agent_id: authorization.agent_id,
            agent_account_id: authorization.account.id.clone(),
            amount_cents: authorization.amount_cents,
            currency: Currency::Usd,
            merchant: authorization.merchant,
            task_id: Some(authorization.reason),
            rail: PaymentRailKind::FiatMock,
            destination: PaymentDestination::FiatAccount {
                account_ref: "demo-merchant-account".to_string(),
            },
            memo: Some("Hubu demo payment".to_string()),
        };

        let payment_audit_request = payment_request.clone();
        let payment_result = state
            .payments
            .lock()
            .map_err(|_| anyhow!("payment manager lock poisoned"))?
            .submit_payment(payment_request);

        match payment_result {
            Ok(payment) => {
                state
                    .payment_attempts
                    .lock()
                    .map_err(|_| anyhow!("payment attempt store lock poisoned"))?
                    .save_payment_attempt(&payment_audit_request, &payment)?;

                if payment.status == PaymentStatus::Succeeded {
                    let used_token = state
                        .spend
                        .lock()
                        .map_err(|_| anyhow!("spend manager lock poisoned"))?
                        .auth_token_record(&payment_audit_request.spend_auth_token_id)
                        .ok_or_else(|| anyhow!("used spend auth token was not recorded"))?;
                    state
                        .governance
                        .lock()
                        .map_err(|_| anyhow!("governance store lock poisoned"))?
                        .update_spend_auth_token(&used_token)?;
                }

                let hold_update = {
                    let mut budgets = state
                        .budgets
                        .lock()
                        .map_err(|_| anyhow!("budget manager lock poisoned"))?;
                    let hold_update = if payment.status == PaymentStatus::Succeeded {
                        BudgetHoldUpdate::Settled(budgets.settle_budget(&reservation.hold.id)?)
                    } else {
                        BudgetHoldUpdate::Released(budgets.release_budget(&reservation.hold.id)?)
                    };
                    let (hold, balance) = match &hold_update {
                        BudgetHoldUpdate::Settled(response) => (&response.hold, &response.balance),
                        BudgetHoldUpdate::Released(response) => (&response.hold, &response.balance),
                    };
                    state
                        .governance
                        .lock()
                        .map_err(|_| anyhow!("governance store lock poisoned"))?
                        .update_budget_hold(hold, balance)?;
                    hold_update
                };

                log_event(
                    "info",
                    "payment_submitted_for_spend",
                    json!({
                        "decision_id": authorization.evaluation.decision_id.to_string(),
                        "payment_id": payment.payment_id.to_string(),
                        "payment_status": match payment.status {
                            PaymentStatus::Succeeded => "succeeded",
                            PaymentStatus::Failed => "failed",
                        },
                        "ledger_transaction_id": payment.ledger_transaction_id.as_ref().map(ToString::to_string),
                        "rail_reference": payment.rail_reference,
                        "failure_reason": payment.failure_reason,
                    }),
                );
                (
                    Some(budget_hold_response(hold_update)),
                    Some(PaymentHttpResponse {
                        payment_id: payment.payment_id.to_string(),
                        owner_user_id: owner.pub_id,
                        owner_user_name: owner.display_name,
                        account_id: authorization.account_pub_id.clone(),
                        status: match payment.status {
                            PaymentStatus::Succeeded => "succeeded",
                            PaymentStatus::Failed => "failed",
                        }
                        .to_string(),
                        ledger_transaction_id: payment
                            .ledger_transaction_id
                            .map(|id| id.to_string()),
                        rail_reference: payment.rail_reference,
                        failure_reason: payment.failure_reason,
                    }),
                )
            }
            Err(error) => {
                let release = state
                    .budgets
                    .lock()
                    .map_err(|_| anyhow!("budget manager lock poisoned"))?
                    .release_budget(&reservation.hold.id)?;
                state
                    .governance
                    .lock()
                    .map_err(|_| anyhow!("governance store lock poisoned"))?
                    .update_budget_hold(&release.hold, &release.balance)?;
                log_event(
                    "warn",
                    "payment_failed_budget_released",
                    json!({
                        "decision_id": authorization.evaluation.decision_id.to_string(),
                        "hold_id": reservation.hold.id.to_string(),
                        "error": error.to_string(),
                    }),
                );
                return Err(error.into());
            }
        }
    } else {
        (None, None)
    };

    Ok(SpendHttpResponse {
        account_id: authorization.account_pub_id,
        agent_id: authorization.agent_pub_id,
        decision_id: authorization.evaluation.decision_id.to_string(),
        decision: effect_name(authorization.evaluation.evaluation.decision).to_string(),
        reasons: authorization.evaluation.evaluation.reasons,
        auth_token_id: authorization.auth_token_id,
        budget_hold,
        payment,
    })
}

struct AuthorizedSpend {
    user: UserContext,
    account: AgentAccount,
    account_pub_id: String,
    agent_id: AgentId,
    agent_pub_id: String,
    amount_cents: i64,
    merchant: Option<String>,
    reason: String,
    evaluation: hubu_core::spend::SpendEvaluationResponse,
    auth_token_id: Option<String>,
    token: Option<hubu_core::spend::IssuedSpendAuthToken>,
    reservation: Option<ReserveBudgetResponse>,
}

enum SpendAuthorization {
    Authorized(AuthorizedSpend),
    Response(SpendHttpResponse),
}

fn evaluate_and_reserve_spend(
    request: SpendHttpRequest,
    state: &ServerState,
) -> Result<SpendAuthorization> {
    if request.amount_cents <= 0 {
        return Err(anyhow!("spend amount must be positive"));
    }
    reconcile_expired_budget_holds(state)?;

    let user = default_user_context(state)?;
    let account = resolve_agent_account_for_spend(&request, &user, state)?;
    let account_pub_id = account.pub_id.clone();
    let agent_id = account.agent_id.clone();
    let agent_pub_id = registration_agent_pub_id(&agent_id, state)?;
    log_event(
        "info",
        "spend_request_received",
        json!({
            "agent_pub_id": agent_pub_id,
            "account_pub_id": account_pub_id,
            "user_id": user.user_id.to_string(),
            "amount_cents": request.amount_cents,
            "currency": Currency::Usd.to_string(),
            "merchant": request.merchant.clone(),
            "reason": request.reason.clone(),
        }),
    );
    let policy = state
        .policies
        .lock()
        .map_err(|_| anyhow!("policy store lock poisoned"))?
        .get(&(user.user_id.clone(), agent_id.clone()))
        .cloned()
        .ok_or_else(|| anyhow!("no policy found for agent {agent_pub_id}"))?;

    let spend_request = SpendRequest {
        amount_cents: request.amount_cents,
        currency: Currency::Usd,
        owner_user_id: user.user_id.clone(),
        agent_id: agent_id.clone(),
        agent_account_id: account.id.clone(),
        merchant: request.merchant.clone(),
        category: None,
        task_id: Some(request.reason.clone()),
    };

    let (evaluation, token_record) = {
        let mut spend = state
            .spend
            .lock()
            .map_err(|_| anyhow!("spend manager lock poisoned"))?;
        let evaluation = spend.evaluate_spend(&user, spend_request.clone(), &policy)?;
        let decision_record = spend
            .decision_record(&evaluation.decision_id)
            .ok_or_else(|| anyhow!("spend decision was not recorded"))?;
        let token_record = evaluation
            .auth_token
            .as_ref()
            .and_then(|token| spend.auth_token_record(&token.id));
        drop(spend);

        let mut governance = state
            .governance
            .lock()
            .map_err(|_| anyhow!("governance store lock poisoned"))?;
        governance.save_spend_decision(&decision_record)?;
        (evaluation, token_record)
    };

    let auth_token_id = evaluation
        .auth_token
        .as_ref()
        .map(|token| token.id.to_string());
    log_event(
        "info",
        "spend_policy_evaluated",
        json!({
            "agent_pub_id": agent_pub_id,
            "agent_id": agent_id.to_string(),
            "user_id": user.user_id.to_string(),
            "decision_id": evaluation.decision_id.to_string(),
            "decision": effect_name(evaluation.evaluation.decision),
            "policy_id": evaluation.evaluation.policy_id,
            "policy_version": evaluation.evaluation.policy_version,
            "auth_token_issued": auth_token_id.is_some(),
        }),
    );
    let token = evaluation.auth_token.clone();
    let reservation = if let Some(token) = token.as_ref() {
        let budget_id = budget_id_for_spend_reservation(&request, &user, &agent_id, state)?;
        let reservation = {
            let mut budgets = state
                .budgets
                .lock()
                .map_err(|_| anyhow!("budget manager lock poisoned"))?;
            let reservation = match budgets.reserve_budget(ReserveBudgetRequest {
                budget_id: budget_id.clone(),
                spend_decision_id: evaluation.decision_id.clone(),
                amount_cents: request.amount_cents,
                currency: Currency::Usd,
                expires_at: token.expires_at,
            }) {
                Ok(reservation) => reservation,
                Err(BudgetManagerError::InsufficientRemainingBudget) => {
                    log_event(
                        "warn",
                        "spend_budget_denied",
                        json!({
                            "agent_pub_id": agent_pub_id,
                            "agent_id": agent_id.to_string(),
                            "account_pub_id": account_pub_id,
                            "user_id": user.user_id.to_string(),
                            "decision_id": evaluation.decision_id.to_string(),
                            "budget_id": budget_id.to_string(),
                            "amount_cents": request.amount_cents,
                            "reason": "insufficient_remaining_budget",
                        }),
                    );
                    return Ok(SpendAuthorization::Response(SpendHttpResponse {
                        account_id: account_pub_id,
                        agent_id: agent_pub_id,
                        decision_id: evaluation.decision_id.to_string(),
                        decision: "deny".to_string(),
                        reasons: vec!["budget does not have enough remaining balance".to_string()],
                        auth_token_id: None,
                        budget_hold: None,
                        payment: None,
                    }));
                }
                Err(error) => return Err(error.into()),
            };
            drop(budgets);
            if let Some(token_record) = &token_record {
                if let Err(error) = state
                    .governance
                    .lock()
                    .map_err(|_| anyhow!("governance store lock poisoned"))?
                    .save_spend_auth_token(token_record)
                {
                    let release = state
                        .budgets
                        .lock()
                        .map_err(|_| anyhow!("budget manager lock poisoned"))?
                        .release_budget(&reservation.hold.id)?;
                    log_event(
                        "warn",
                        "spend_budget_reservation_released",
                        json!({
                            "decision_id": evaluation.decision_id.to_string(),
                            "hold_id": reservation.hold.id.to_string(),
                            "remaining_amount_cents": release.balance.remaining_amount_cents,
                            "error": error.to_string(),
                        }),
                    );
                    return Err(error.into());
                }
            }
            state
                .governance
                .lock()
                .map_err(|_| anyhow!("governance store lock poisoned"))?
                .save_budget_hold(&reservation.hold, &reservation.balance)?;
            reservation
        };
        log_event(
            "info",
            "budget_reserved_for_spend",
            json!({
                "budget_id": reservation.hold.budget_id.to_string(),
                "hold_id": reservation.hold.id.to_string(),
                "decision_id": evaluation.decision_id.to_string(),
                "amount_cents": reservation.hold.amount_cents,
                "remaining_amount_cents": reservation.balance.remaining_amount_cents,
                "frozen_amount_cents": reservation.balance.frozen_amount_cents,
            }),
        );
        Some(reservation)
    } else {
        None
    };

    Ok(SpendAuthorization::Authorized(AuthorizedSpend {
        user,
        account,
        account_pub_id,
        agent_id,
        agent_pub_id,
        amount_cents: request.amount_cents,
        merchant: request.merchant,
        reason: request.reason,
        evaluation,
        auth_token_id,
        token,
        reservation,
    }))
}

fn generate_image(body: String, state: &ServerState) -> Result<GenerateImageHttpResponse> {
    let request: GenerateImageHttpRequest = serde_json::from_str(&body)?;
    if request.prompt.trim().is_empty() {
        return Err(anyhow!("image prompt cannot be empty"));
    }

    let (provider, model) = state
        .image_provider
        .resolve(request.provider, request.model)?;
    let image_adapter = state.image_provider.adapter()?;
    let user = default_user_context(state)?;
    let token_id = SpendAuthTokenId::from_str(&request.spend_auth_token_id)
        .map_err(|error| anyhow!("invalid spend_auth_token_id: {error}"))?;
    let (token_record, decision_record) = state
        .spend
        .lock()
        .map_err(|_| anyhow!("spend manager lock poisoned"))?
        .decision_for_auth_token(&token_id)?;

    if token_record.owner_user_id != user.user_id || decision_record.owner_user_id != user.user_id {
        return Err(anyhow!("spend authorization is not owned by resolved user"));
    }
    if token_record.used_at.is_some() {
        return Err(anyhow!("spend authorization has already been used"));
    }
    if token_record.revoked_at.is_some() {
        return Err(anyhow!("spend authorization has been revoked"));
    }
    if token_record.expires_at <= Utc::now() {
        return Err(anyhow!("spend authorization has expired"));
    }

    let authorized_spend = decision_record.request;
    if authorized_spend.merchant.as_deref() != Some(state.image_provider.merchant.as_str()) {
        return Err(anyhow!(
            "spend authorization is not scoped to the configured image proxy merchant"
        ));
    }
    let account = state
        .registration
        .lock()
        .map_err(|_| anyhow!("registration manager lock poisoned"))?
        .account_for_agent(&authorized_spend.agent_id)?
        .ok_or_else(|| anyhow!("authorized agent account was not found"))?;
    if account.id != authorized_spend.agent_account_id {
        return Err(anyhow!(
            "authorized agent account does not match registration"
        ));
    }
    let account_pub_id = account.pub_id;
    let owner = owner_metadata_for_user_id(&user.user_id, state)?;
    let frozen_hold = state
        .budgets
        .lock()
        .map_err(|_| anyhow!("budget manager lock poisoned"))?
        .get_budget_hold_by_spend_decision(&decision_record.id)
        .ok_or_else(|| anyhow!("authorized spend has no reserved budget hold"))?;

    let artifact_id = token_record.id.to_string();
    let image_output = match image_adapter.generate(ImageGenerationRequest {
        provider: &provider,
        model: &model,
        prompt: &request.prompt,
        artifact_id: &artifact_id,
    }) {
        Ok(output) => output,
        Err(error) => {
            let release = {
                let mut budgets = state
                    .budgets
                    .lock()
                    .map_err(|_| anyhow!("budget manager lock poisoned"))?;
                budgets.release_budget(&frozen_hold.id)?
            };
            state
                .governance
                .lock()
                .map_err(|_| anyhow!("governance store lock poisoned"))?
                .update_budget_hold(&release.hold, &release.balance)?;
            log_event(
                "warn",
                "image_provider_generation_failed",
                json!({
                    "provider": provider,
                    "model": model,
                    "spend_auth_token_id": token_record.id.to_string(),
                    "hold_id": frozen_hold.id.to_string(),
                    "budget_id": frozen_hold.budget_id.to_string(),
                    "amount_cents": authorized_spend.amount_cents,
                    "currency": authorized_spend.currency.to_string(),
                    "failure": error.to_string(),
                }),
            );
            return Err(anyhow!("image provider generation failed: {error}"));
        }
    };

    let payment_request = PaymentRequest {
        idempotency_key: format!("image-proxy:{}", token_record.id),
        spend_auth_token_id: token_record.id.clone(),
        owner_user_id: user.user_id.clone(),
        agent_id: authorized_spend.agent_id,
        agent_account_id: authorized_spend.agent_account_id,
        amount_cents: authorized_spend.amount_cents,
        currency: authorized_spend.currency,
        merchant: authorized_spend.merchant,
        task_id: authorized_spend.task_id,
        rail: PaymentRailKind::FiatMock,
        destination: PaymentDestination::FiatAccount {
            account_ref: format!("{provider}:{model}"),
        },
        memo: Some("Hubu image generation proxy".to_string()),
    };

    let payment_audit_request = payment_request.clone();
    let payment = state
        .payments
        .lock()
        .map_err(|_| anyhow!("payment manager lock poisoned"))?
        .submit_payment(payment_request)?;
    state
        .payment_attempts
        .lock()
        .map_err(|_| anyhow!("payment attempt store lock poisoned"))?
        .save_payment_attempt(&payment_audit_request, &payment)?;

    let used_token = state
        .spend
        .lock()
        .map_err(|_| anyhow!("spend manager lock poisoned"))?
        .auth_token_record(&payment_audit_request.spend_auth_token_id)
        .ok_or_else(|| anyhow!("used spend auth token was not recorded"))?;
    state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?
        .update_spend_auth_token(&used_token)?;

    let hold_update = {
        let mut budgets = state
            .budgets
            .lock()
            .map_err(|_| anyhow!("budget manager lock poisoned"))?;
        let hold_update = if payment.status == PaymentStatus::Succeeded {
            BudgetHoldUpdate::Settled(budgets.settle_budget(&frozen_hold.id)?)
        } else {
            BudgetHoldUpdate::Released(budgets.release_budget(&frozen_hold.id)?)
        };
        let (hold, balance) = match &hold_update {
            BudgetHoldUpdate::Settled(response) => (&response.hold, &response.balance),
            BudgetHoldUpdate::Released(response) => (&response.hold, &response.balance),
        };
        state
            .governance
            .lock()
            .map_err(|_| anyhow!("governance store lock poisoned"))?
            .update_budget_hold(hold, balance)?;
        hold_update
    };

    if payment.status != PaymentStatus::Succeeded {
        return Err(anyhow!(
            "image generation proxy payment failed: {}",
            payment
                .failure_reason
                .clone()
                .unwrap_or_else(|| "unknown payment failure".to_string())
        ));
    }

    log_event(
        "info",
        "image_model_call_proxied",
        json!({
            "provider": provider,
            "model": model,
            "spend_auth_token_id": token_record.id.to_string(),
            "payment_id": payment.payment_id.to_string(),
            "output_ref": image_output.output_ref.clone(),
            "amount_cents": payment.amount_cents,
            "currency": payment.currency.to_string(),
            "provider_api_key_configured": state.image_provider.has_api_key(),
        }),
    );

    Ok(GenerateImageHttpResponse {
        provider,
        model,
        output_ref: image_output.output_ref,
        spend_auth_token_id: token_record.id.to_string(),
        payment: PaymentHttpResponse {
            payment_id: payment.payment_id.to_string(),
            owner_user_id: owner.pub_id,
            owner_user_name: owner.display_name,
            account_id: account_pub_id,
            status: "succeeded".to_string(),
            ledger_transaction_id: payment.ledger_transaction_id.map(|id| id.to_string()),
            rail_reference: payment.rail_reference,
            failure_reason: None,
        },
        budget_hold: budget_hold_response(hold_update),
    })
}

fn demo_image_svg(provider: &str, model: &str, prompt: &str) -> String {
    let prompt_preview = prompt.chars().take(120).collect::<String>();
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024" role="img" aria-labelledby="title desc">
  <title id="title">Project Hubu demo logo</title>
  <desc id="desc">Generated by Hubu demo provider {provider} using {model}. Prompt: {prompt}</desc>
  <rect width="1024" height="1024" rx="144" fill="#0f172a"/>
  <circle cx="512" cy="512" r="312" fill="#14b8a6"/>
  <circle cx="512" cy="512" r="244" fill="#f8fafc"/>
  <path d="M330 318h96v156h172V318h96v388h-96V552H426v154h-96z" fill="#0f172a"/>
  <path d="M306 754h412v58H306z" fill="#f59e0b"/>
  <text x="512" y="894" text-anchor="middle" font-family="Arial, Helvetica, sans-serif" font-size="48" font-weight="700" fill="#f8fafc">HUBU</text>
  <text x="512" y="950" text-anchor="middle" font-family="Arial, Helvetica, sans-serif" font-size="24" fill="#cbd5e1">{prompt}</text>
</svg>
"##,
        provider = escape_xml(provider),
        model = escape_xml(model),
        prompt = escape_xml(&prompt_preview),
    )
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

enum BudgetHoldUpdate {
    Settled(SettleBudgetResponse),
    Released(ReleaseBudgetResponse),
}

fn reconcile_expired_budget_holds(state: &ServerState) -> Result<()> {
    let expired = state
        .budgets
        .lock()
        .map_err(|_| anyhow!("budget manager lock poisoned"))?
        .expire_overdue_budget_holds(Utc::now())?;
    if expired.is_empty() {
        return Ok(());
    }

    let mut governance = state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?;
    for response in expired {
        governance.update_budget_hold(&response.hold, &response.balance)?;
    }
    Ok(())
}

fn active_budget_id_for_user(user_id: &UserId, state: &ServerState) -> Result<BudgetId> {
    let now = Utc::now();
    state
        .budgets
        .lock()
        .map_err(|_| anyhow!("budget manager lock poisoned"))?
        .get_budgets_by_user_id(user_id)
        .into_iter()
        .find(|budget| {
            budget.budget.currency == Currency::Usd && budget.budget.period.contains(now)
        })
        .map(|budget| budget.budget.id)
        .ok_or_else(|| anyhow!("no active USD budget found for current user"))
}

fn budget_id_for_spend_reservation(
    request: &SpendHttpRequest,
    user: &UserContext,
    agent_id: &AgentId,
    state: &ServerState,
) -> Result<BudgetId> {
    let Some(budget_id) = request.budget_id.as_deref() else {
        return active_budget_id_for_user(&user.user_id, state);
    };
    let requested_budget_id = BudgetId::from_str(budget_id)
        .map_err(|error| anyhow!("invalid budget_id `{budget_id}`: {error}"))?;
    let now = Utc::now();
    let budgets = state
        .budgets
        .lock()
        .map_err(|_| anyhow!("budget manager lock poisoned"))?;
    let budget = budgets
        .get_budgets_by_user_id(&user.user_id)
        .into_iter()
        .chain(budgets.get_budgets_by_agent_id(agent_id))
        .find(|budget| budget.budget.id == requested_budget_id)
        .ok_or_else(|| anyhow!("budget_id is not available to this spend request"))?;
    if budget.budget.currency != Currency::Usd || !budget.budget.period.contains(now) {
        return Err(anyhow!("budget_id is not an active USD budget"));
    }
    Ok(requested_budget_id)
}

fn resolve_agent_account_for_spend(
    request: &SpendHttpRequest,
    user: &UserContext,
    state: &ServerState,
) -> Result<AgentAccount> {
    if request.account_id.is_some() == request.agent_id.is_some() {
        return Err(anyhow!(
            "spend request must include exactly one of account_id or agent_id"
        ));
    }

    let registration = state
        .registration
        .lock()
        .map_err(|_| anyhow!("registration manager lock poisoned"))?;

    let account = if let Some(account_pub_id) = request.account_id.as_deref() {
        registration
            .account_for_pub_id(account_pub_id)?
            .ok_or_else(|| anyhow!("unknown public account id {account_pub_id}"))?
    } else if let Some(agent_pub_id) = request.agent_id.as_deref() {
        let agent_id = registration
            .agent_id_for_pub_id(agent_pub_id)?
            .ok_or_else(|| anyhow!("unknown public agent id {agent_pub_id}"))?;
        registration
            .account_for_agent(&agent_id)?
            .ok_or_else(|| anyhow!("no account found for agent {agent_pub_id}"))?
    } else {
        return Err(anyhow!("spend request must include account_id or agent_id"));
    };

    if account.owner_user_id != user.user_id {
        return Err(anyhow!(
            "account {} is not owned by resolved user",
            account.pub_id
        ));
    }

    if account.account_status != AccountStatus::Active {
        return Err(anyhow!("account {} is not active", account.pub_id));
    }

    Ok(account)
}

fn registration_agent_pub_id(agent_id: &AgentId, state: &ServerState) -> Result<String> {
    let registration = state
        .registration
        .lock()
        .map_err(|_| anyhow!("registration manager lock poisoned"))?;
    registration
        .agent_for_id(agent_id)?
        .map(|agent| agent.pub_id)
        .ok_or_else(|| anyhow!("agent index is stale for account owner"))
}

fn budget_hold_response(update: BudgetHoldUpdate) -> BudgetHoldHttpResponse {
    match update {
        BudgetHoldUpdate::Settled(response) => BudgetHoldHttpResponse {
            hold_id: response.hold.id.to_string(),
            budget_id: response.hold.budget_id.to_string(),
            status: "settled".to_string(),
            amount_cents: response.hold.amount_cents,
            consumed_amount_cents: response.balance.consumed_amount_cents,
            frozen_amount_cents: response.balance.frozen_amount_cents,
            remaining_amount_cents: response.balance.remaining_amount_cents,
        },
        BudgetHoldUpdate::Released(response) => BudgetHoldHttpResponse {
            hold_id: response.hold.id.to_string(),
            budget_id: response.hold.budget_id.to_string(),
            status: "released".to_string(),
            amount_cents: response.hold.amount_cents,
            consumed_amount_cents: response.balance.consumed_amount_cents,
            frozen_amount_cents: response.balance.frozen_amount_cents,
            remaining_amount_cents: response.balance.remaining_amount_cents,
        },
    }
}

fn frozen_budget_hold_response(response: ReserveBudgetResponse) -> BudgetHoldHttpResponse {
    BudgetHoldHttpResponse {
        hold_id: response.hold.id.to_string(),
        budget_id: response.hold.budget_id.to_string(),
        status: "frozen".to_string(),
        amount_cents: response.hold.amount_cents,
        consumed_amount_cents: response.balance.consumed_amount_cents,
        frozen_amount_cents: response.balance.frozen_amount_cents,
        remaining_amount_cents: response.balance.remaining_amount_cents,
    }
}

fn budget_response(budget: BudgetWithBalance) -> BudgetHttpResponse {
    BudgetHttpResponse {
        budget_id: budget.budget.id.to_string(),
        scope: budget_scope_name(&budget.budget.scope).to_string(),
        amount_limit_cents: budget.budget.amount_limit_cents,
        currency: budget.budget.currency.to_string(),
        starting_at: budget.budget.period.starting_at.to_rfc3339(),
        ending_before: budget
            .budget
            .period
            .ending_before
            .map(|ending_before| ending_before.to_rfc3339()),
        status: budget_status_name(budget.budget.status).to_string(),
        consumed_amount_cents: budget.balance.consumed_amount_cents,
        frozen_amount_cents: budget.balance.frozen_amount_cents,
        remaining_amount_cents: budget.balance.remaining_amount_cents,
    }
}

fn budget_scope_name(scope: &BudgetScope) -> &'static str {
    match scope {
        BudgetScope::User(_) => "user",
        BudgetScope::Agent(_) => "agent",
        BudgetScope::Task(_) => "task",
    }
}

fn budget_status_name(status: hubu_core::budget::BudgetStatus) -> &'static str {
    match status {
        hubu_core::budget::BudgetStatus::Active => "active",
        hubu_core::budget::BudgetStatus::Exhausted => "exhausted",
        hubu_core::budget::BudgetStatus::Expired => "expired",
        hubu_core::budget::BudgetStatus::Revoked => "revoked",
    }
}

fn parse_optional_datetime(value: Option<String>) -> Result<Option<DateTime<Utc>>> {
    value
        .map(|value| {
            DateTime::parse_from_rfc3339(&value)
                .map(|parsed| parsed.with_timezone(&Utc))
                .with_context(|| format!("parse datetime `{value}`"))
        })
        .transpose()
}

fn resolve_agent_id_for_user(
    agent_pub_id: &str,
    user: &UserContext,
    state: &ServerState,
) -> Result<AgentId> {
    let registration = state
        .registration
        .lock()
        .map_err(|_| anyhow!("registration manager lock poisoned"))?;
    let agent_id = registration
        .agent_id_for_pub_id(agent_pub_id)?
        .ok_or_else(|| anyhow!("unknown public agent id {agent_pub_id}"))?;
    let agent = registration
        .agent_for_id(&agent_id)?
        .ok_or_else(|| anyhow!("agent index is stale for {agent_pub_id}"))?;
    if agent.owner_user_id != user.user_id {
        return Err(anyhow!(
            "agent {agent_pub_id} is not owned by resolved user"
        ));
    }
    Ok(agent_id)
}

fn default_user_context(state: &ServerState) -> Result<UserContext> {
    let mut users = state
        .users
        .lock()
        .map_err(|_| anyhow!("user manager lock poisoned"))?;
    users.ensure_default_user()?;
    Ok(users.default_user_context()?)
}

fn default_user(state: &ServerState) -> Result<User> {
    let mut users = state
        .users
        .lock()
        .map_err(|_| anyhow!("user manager lock poisoned"))?;
    Ok(users.ensure_default_user()?)
}

fn owner_metadata_for_user_id(user_id: &UserId, state: &ServerState) -> Result<OwnerHttpMetadata> {
    let users = state
        .users
        .lock()
        .map_err(|_| anyhow!("user manager lock poisoned"))?;
    let user = users
        .user_for_id(user_id)?
        .ok_or_else(|| anyhow!("unknown owner user {user_id}"))?;

    Ok(OwnerHttpMetadata {
        pub_id: user.pub_id,
        display_name: user.display_name,
    })
}

fn list_ledger(state: &ServerState) -> Result<LedgerHttpResponse> {
    let payments = state
        .payments
        .lock()
        .map_err(|_| anyhow!("payment manager lock poisoned"))?;
    let transactions = payments
        .ledger()
        .list_transactions()?
        .into_iter()
        .map(|transaction| ledger_transaction_response(payments.ledger(), transaction, state))
        .collect::<Result<Vec<_>>>()?;

    Ok(LedgerHttpResponse { transactions })
}

fn ledger_transaction_response(
    ledger: &SqliteLedger,
    transaction: LedgerTransaction,
    state: &ServerState,
) -> Result<LedgerTransactionHttpResponse> {
    let transaction_owner = owner_metadata_for_user_id(&transaction.owner_user_id, state)?;
    let entries = ledger
        .entries_for_transaction(&transaction.id)?
        .into_iter()
        .map(|entry| {
            let entry_owner = owner_metadata_for_user_id(&entry.owner_user_id, state)?;
            Ok(LedgerEntryHttpResponse {
                id: entry.id.to_string(),
                owner_user_id: entry_owner.pub_id,
                owner_user_name: entry_owner.display_name,
                account_id: entry.account_id.to_string(),
                direction: match entry.direction {
                    hubu_wallet::LedgerDirection::Debit => "debit",
                    hubu_wallet::LedgerDirection::Credit => "credit",
                }
                .to_string(),
                amount_cents: entry.amount_cents,
                currency: entry.currency.to_string(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(LedgerTransactionHttpResponse {
        id: transaction.id.to_string(),
        owner_user_id: transaction_owner.pub_id,
        owner_user_name: transaction_owner.display_name,
        external_ref: transaction.external_ref,
        description: transaction.description,
        created_at: transaction.created_at.to_rfc3339(),
        entries,
    })
}

fn effect_name(effect: Effect) -> &'static str {
    match effect {
        Effect::Allow => "allow",
        Effect::Deny => "deny",
        Effect::NeedsApproval => "needs_approval",
    }
}

fn to_json<T: Serialize>(value: T) -> Value {
    serde_json::to_value(value).expect("HTTP response should serialize")
}

struct HttpRequest {
    method: String,
    path: String,
    body: String,
}

struct HttpResponse {
    status: u16,
    body: Value,
}

struct OwnerHttpMetadata {
    pub_id: String,
    display_name: String,
}

fn parse_request(raw: &str) -> Result<HttpRequest> {
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow!("invalid HTTP request"))?;
    let mut request_line = head.lines().next().unwrap_or_default().split_whitespace();
    let method = request_line
        .next()
        .ok_or_else(|| anyhow!("missing method"))?
        .to_string();
    let path = request_line
        .next()
        .ok_or_else(|| anyhow!("missing path"))?
        .split('?')
        .next()
        .unwrap_or_default()
        .to_string();

    Ok(HttpRequest {
        method,
        path,
        body: body.to_string(),
    })
}

fn write_response(stream: &mut TcpStream, response: HttpResponse) -> Result<()> {
    let body = response.body.to_string();
    let status_text = if response.status == 200 {
        "OK"
    } else {
        "Bad Request"
    };
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        response.status,
        status_text,
        body.len(),
        body
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_guidance_is_available_for_agents() {
        let path = std::env::temp_dir().join(format!("hubu-api-guidance-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");

        for path in [
            "/registration/guidance",
            "/.well-known/hubu-agent-registration.json",
        ] {
            let response = route(
                HttpRequest {
                    method: "GET".to_string(),
                    path: path.to_string(),
                    body: String::new(),
                },
                &state,
            );

            assert_eq!(response.status, 200);
            assert_eq!(
                response.body["protocol_version"],
                "hubu-agent-registration-v1"
            );
            assert_eq!(response.body["fingerprint"]["algorithm"], "sha256");
            assert_eq!(response.body["signature_policy"], "not_supported");
            assert!(response.body["human_inputs"]
                .as_array()
                .expect("human_inputs should be an array")
                .iter()
                .any(|field| field["name"] == "agent_name"));
            assert!(response.body["identity_payload"]["required"]
                .as_array()
                .expect("identity required fields should be an array")
                .iter()
                .any(|field| field == "owner"));
            assert!(response.body["version_payload"]["required"]
                .as_array()
                .expect("version required fields should be an array")
                .iter()
                .any(|field| field == "identity_fingerprint"));
        }

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn registration_envelope_registers_agent() {
        let path = std::env::temp_dir().join(format!("hubu-api-envelope-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        let user = init(
            json!({
                "display_name": "Alice Example",
                "email": "alice@example.com",
            })
            .to_string(),
            &state,
        )
        .expect("init should create user");
        let envelope = simple_registration_envelope("protocol-agent", "dev", &user.user_id);

        let agent = register_agent(
            serde_json::to_string(&envelope).expect("envelope should serialize"),
            &state,
        )
        .expect("protocol envelope should register");

        assert!(agent.agent_id.starts_with("agt_"));
        assert_eq!(agent.user_id, user.user_id);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn registration_envelope_rejects_tampered_identity_payload() {
        let path = std::env::temp_dir().join(format!("hubu-api-identity-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        let user = init(
            json!({
                "display_name": "Alice Example",
                "email": "alice@example.com",
            })
            .to_string(),
            &state,
        )
        .expect("init should create user");
        let mut envelope = simple_registration_envelope("protocol-agent", "dev", &user.user_id);
        envelope.identity.payload["agent_name"] = json!("different-agent");

        let error = register_agent(
            serde_json::to_string(&envelope).expect("envelope should serialize"),
            &state,
        )
        .expect_err("tampered identity payload should fail");

        assert!(error.to_string().contains("identity fingerprint mismatch"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn registration_envelope_rejects_tampered_version_payload() {
        let path = std::env::temp_dir().join(format!("hubu-api-version-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        let user = init(
            json!({
                "display_name": "Alice Example",
                "email": "alice@example.com",
            })
            .to_string(),
            &state,
        )
        .expect("init should create user");
        let mut envelope = simple_registration_envelope("protocol-agent", "dev", &user.user_id);
        envelope.version.payload["version_label"] = json!("v2");

        let error = register_agent(
            serde_json::to_string(&envelope).expect("envelope should serialize"),
            &state,
        )
        .expect_err("tampered version payload should fail");

        assert!(error.to_string().contains("version fingerprint mismatch"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn registration_envelope_rejects_missing_required_runtime() {
        let path = std::env::temp_dir().join(format!("hubu-api-runtime-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        let user = init(
            json!({
                "display_name": "Alice Example",
                "email": "alice@example.com",
            })
            .to_string(),
            &state,
        )
        .expect("init should create user");
        let mut envelope = simple_registration_envelope("protocol-agent", "dev", &user.user_id);
        envelope
            .version
            .payload
            .as_object_mut()
            .expect("version payload should be an object")
            .remove("runtime");
        envelope.version.fingerprint = fingerprint_payload(&envelope.version.payload);

        let error = register_agent(
            serde_json::to_string(&envelope).expect("envelope should serialize"),
            &state,
        )
        .expect_err("missing runtime should fail");

        assert!(error
            .to_string()
            .contains("version payload missing object field `runtime`"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn registration_envelope_rejects_missing_required_hubu_client() {
        let path = std::env::temp_dir().join(format!("hubu-api-client-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        let user = init(
            json!({
                "display_name": "Alice Example",
                "email": "alice@example.com",
            })
            .to_string(),
            &state,
        )
        .expect("init should create user");
        let mut envelope = simple_registration_envelope("protocol-agent", "dev", &user.user_id);
        envelope
            .version
            .payload
            .as_object_mut()
            .expect("version payload should be an object")
            .remove("hubu_client");
        envelope.version.fingerprint = fingerprint_payload(&envelope.version.payload);

        let error = register_agent(
            serde_json::to_string(&envelope).expect("envelope should serialize"),
            &state,
        )
        .expect_err("missing hubu_client should fail");

        assert!(error
            .to_string()
            .contains("version payload missing object field `hubu_client`"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn registration_envelope_rejects_owner_mismatch() {
        let path = std::env::temp_dir().join(format!("hubu-api-owner-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        init(
            json!({
                "display_name": "Alice Example",
                "email": "alice@example.com",
            })
            .to_string(),
            &state,
        )
        .expect("init should create user");
        let envelope = simple_registration_envelope("protocol-agent", "dev", "usr_wrongowner");

        let error = register_agent(
            serde_json::to_string(&envelope).expect("envelope should serialize"),
            &state,
        )
        .expect_err("owner mismatch should fail");

        assert!(error
            .to_string()
            .contains("owner.pub_id does not match active Hubu user"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn init_user_is_shown_on_payment_and_ledger_records() {
        let path = std::env::temp_dir().join(format!("hubu-api-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        let user = init(
            json!({
                "display_name": "Alice Example",
                "email": "alice@example.com",
            })
            .to_string(),
            &state,
        )
        .expect("init should create an explicit user");

        let agent = register_agent(
            json!({
                "name": "settlement-agent",
                "version": "v1",
            })
            .to_string(),
            &state,
        )
        .expect("agent should register under initialized user");
        assert_eq!(agent.user_id, user.user_id);

        add_policy(
            json!({
                "agent_id": agent.agent_id,
                "daily_limit_cents": 5_000,
            })
            .to_string(),
            &state,
        )
        .expect("policy should be added under initialized user");

        create_budget(
            json!({
                "amount_cents": 10_000,
            })
            .to_string(),
            &state,
        )
        .expect("budget should be created for initialized user");

        let spend = spend(
            json!({
                "agent_id": agent.agent_id,
                "amount_cents": 2_500,
                "reason": "test purchase",
                "merchant": "Acme Cafe",
            })
            .to_string(),
            &state,
        )
        .expect("spend should be approved and paid");
        let payment = spend.payment.expect("allowed spend should pay");
        assert_eq!(payment.owner_user_id, user.user_id);
        assert_eq!(payment.owner_user_name, "Alice Example");
        let budget_hold = spend
            .budget_hold
            .expect("allowed spend should reserve budget");
        assert_eq!(budget_hold.status, "settled");
        assert_eq!(budget_hold.consumed_amount_cents, 2_500);
        assert_eq!(budget_hold.frozen_amount_cents, 0);
        assert_eq!(budget_hold.remaining_amount_cents, 7_500);

        let ledger = list_ledger(&state).expect("ledger should list");
        assert_eq!(ledger.transactions.len(), 1);
        let transaction = &ledger.transactions[0];
        assert_eq!(transaction.owner_user_id, user.user_id);
        assert_eq!(transaction.owner_user_name, "Alice Example");
        assert!(transaction.entries.iter().all(|entry| {
            entry.owner_user_id == user.user_id && entry.owner_user_name == "Alice Example"
        }));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn spend_can_use_registered_account_id_as_financial_anchor() {
        let path = std::env::temp_dir().join(format!("hubu-api-account-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        let user = init(
            json!({
                "display_name": "Alice Example",
                "email": "alice@example.com",
            })
            .to_string(),
            &state,
        )
        .expect("init should create an explicit user");

        let agent = register_agent(
            json!({
                "name": "account-spend-agent",
                "version": "v1",
            })
            .to_string(),
            &state,
        )
        .expect("agent should register under initialized user");

        add_policy(
            json!({
                "agent_id": agent.agent_id,
                "daily_limit_cents": 5_000,
            })
            .to_string(),
            &state,
        )
        .expect("policy should be added under initialized user");

        create_budget(
            json!({
                "amount_cents": 10_000,
            })
            .to_string(),
            &state,
        )
        .expect("budget should be created for initialized user");

        let spend = spend(
            json!({
                "account_id": agent.account_id,
                "amount_cents": 2_500,
                "reason": "account anchored purchase",
                "merchant": "Acme Cafe",
            })
            .to_string(),
            &state,
        )
        .expect("spend should be approved and paid from account");

        assert_eq!(spend.account_id, agent.account_id);
        assert_eq!(spend.agent_id, agent.agent_id);
        let payment = spend.payment.expect("allowed spend should pay");
        assert_eq!(payment.account_id, agent.account_id);
        assert_eq!(payment.owner_user_id, user.user_id);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn authorize_spend_freezes_budget_without_payment_or_ledger() {
        let path =
            std::env::temp_dir().join(format!("hubu-api-authorize-spend-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        init(
            json!({
                "display_name": "Alice Example",
                "email": "alice@example.com",
            })
            .to_string(),
            &state,
        )
        .expect("init should create an explicit user");

        let agent = register_agent(
            json!({
                "name": "logo-design-agent",
                "version": "v1",
            })
            .to_string(),
            &state,
        )
        .expect("agent should register under initialized user");

        add_policy(
            json!({
                "agent_id": agent.agent_id,
                "daily_limit_cents": 500,
            })
            .to_string(),
            &state,
        )
        .expect("policy should allow the logo budget");

        let logo_budget = create_budget(
            json!({
                "agent_id": agent.agent_id,
                "amount_cents": 500,
            })
            .to_string(),
            &state,
        )
        .expect("agent logo budget should be created");
        let logo_budget_id = logo_budget.budget.budget_id.clone();

        let authorization = authorize_spend(
            json!({
                "agent_id": agent.agent_id,
                "budget_id": logo_budget_id,
                "amount_cents": 500,
                "reason": "Generate Project Hubu logo",
                "merchant": "hubu-model-proxy",
            })
            .to_string(),
            &state,
        )
        .expect("spend should authorize");

        assert_eq!(authorization.decision, "allow");
        assert!(authorization.auth_token_id.is_some());
        assert!(authorization.payment.is_none());
        let budget_hold = authorization
            .budget_hold
            .expect("allowed authorization should reserve budget");
        assert_eq!(budget_hold.status, "frozen");
        assert_eq!(budget_hold.amount_cents, 500);
        assert_eq!(budget_hold.consumed_amount_cents, 0);
        assert_eq!(budget_hold.frozen_amount_cents, 500);
        assert_eq!(budget_hold.remaining_amount_cents, 0);

        let ledger = list_ledger(&state).expect("ledger should list");
        assert!(ledger.transactions.is_empty());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn list_budgets_includes_registered_agent_budgets() {
        let path = std::env::temp_dir().join(format!(
            "hubu-api-list-agent-budgets-{}.sqlite",
            UserId::new()
        ));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        init(
            json!({
                "display_name": "Alice Example",
                "email": "alice@example.com",
            })
            .to_string(),
            &state,
        )
        .expect("init should create an explicit user");

        let agent = register_agent(
            json!({
                "name": "logo-design-agent",
                "version": "v1",
            })
            .to_string(),
            &state,
        )
        .expect("agent should register under initialized user");

        create_budget(
            json!({
                "amount_cents": 1_000,
            })
            .to_string(),
            &state,
        )
        .expect("user budget should be created");
        create_budget(
            json!({
                "agent_id": agent.agent_id,
                "amount_cents": 500,
            })
            .to_string(),
            &state,
        )
        .expect("agent budget should be created");

        let budgets = list_budgets(&state).expect("budgets should list");
        let mut scopes = budgets
            .budgets
            .iter()
            .map(|budget| budget.scope.clone())
            .collect::<Vec<_>>();
        scopes.sort();

        assert_eq!(budgets.budgets.len(), 2);
        assert_eq!(scopes, vec!["agent".to_string(), "user".to_string()]);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn image_proxy_consumes_authorized_spend_and_settles_budget() {
        let path =
            std::env::temp_dir().join(format!("hubu-api-image-proxy-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        let user = init(
            json!({
                "display_name": "Alice Example",
                "email": "alice@example.com",
            })
            .to_string(),
            &state,
        )
        .expect("init should create an explicit user");

        let agent = register_agent(
            json!({
                "name": "logo-design-agent",
                "version": "v1",
            })
            .to_string(),
            &state,
        )
        .expect("agent should register under initialized user");

        add_policy(
            json!({
                "agent_id": agent.agent_id,
                "daily_limit_cents": 500,
            })
            .to_string(),
            &state,
        )
        .expect("policy should allow the logo budget");

        let logo_budget = create_budget(
            json!({
                "agent_id": agent.agent_id,
                "amount_cents": 500,
            })
            .to_string(),
            &state,
        )
        .expect("agent logo budget should be created");
        let logo_budget_id = logo_budget.budget.budget_id.clone();

        let authorization = authorize_spend(
            json!({
                "agent_id": agent.agent_id,
                "budget_id": logo_budget_id,
                "amount_cents": 500,
                "reason": "Generate Project Hubu logo",
                "merchant": "hubu-model-proxy",
            })
            .to_string(),
            &state,
        )
        .expect("spend should authorize");
        let auth_token_id = authorization
            .auth_token_id
            .expect("allowed spend should issue a token");

        let generated = generate_image(
            json!({
                "spend_auth_token_id": auth_token_id,
                "prompt": "Create a crisp logo for Project Hubu",
                "provider": "hubu-demo",
                "model": "demo-image-v1",
            })
            .to_string(),
            &state,
        )
        .expect("image proxy should consume authorization");

        assert_eq!(generated.provider, "hubu-demo");
        assert_eq!(generated.model, "demo-image-v1");
        assert!(generated.output_ref.starts_with("file://"));
        let output_path = generated
            .output_ref
            .strip_prefix("file://")
            .expect("output ref should use file URI");
        let output_svg =
            std::fs::read_to_string(output_path).expect("demo image artifact should be readable");
        assert!(output_svg.contains("Project Hubu demo logo"));
        assert!(output_svg.contains("Create a crisp logo for Project Hubu"));
        assert_eq!(generated.payment.status, "succeeded");
        assert_eq!(generated.payment.owner_user_id, user.user_id);
        assert!(generated.payment.ledger_transaction_id.is_some());
        assert_eq!(generated.budget_hold.status, "settled");
        assert_eq!(
            generated.budget_hold.budget_id,
            logo_budget.budget.budget_id
        );
        assert_eq!(generated.budget_hold.consumed_amount_cents, 500);
        assert_eq!(generated.budget_hold.frozen_amount_cents, 0);
        assert_eq!(generated.budget_hold.remaining_amount_cents, 0);

        let ledger = list_ledger(&state).expect("ledger should list");
        assert_eq!(ledger.transactions.len(), 1);

        let generated_token_id = generated.spend_auth_token_id.clone();
        let reuse_error = generate_image(
            json!({
                "spend_auth_token_id": generated_token_id,
                "prompt": "Try to reuse the authorization",
            })
            .to_string(),
            &state,
        )
        .expect_err("used spend auth token should not be reusable");
        assert!(reuse_error
            .to_string()
            .contains("spend authorization has already been used"));
        std::fs::remove_file(output_path).ok();
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn image_provider_config_rejects_unconfigured_provider_or_model() {
        let config = ImageProviderConfig {
            provider: "hubu-demo".to_string(),
            model: "demo-image-v1".to_string(),
            merchant: "hubu-model-proxy".to_string(),
            api_key: Some("server-side-secret".to_string()),
            output_dir: std::env::temp_dir(),
            adapter_kind: ImageProviderAdapterKind::Demo,
        };

        let resolved = config
            .resolve(None, None)
            .expect("defaults should resolve to configured provider");
        assert_eq!(
            resolved,
            ("hubu-demo".to_string(), "demo-image-v1".to_string())
        );
        assert!(config.has_api_key());

        let error = config
            .resolve(Some("nano-banana".to_string()), None)
            .expect_err("agent should not be able to select an unconfigured provider");
        assert!(error
            .to_string()
            .contains("requested image provider/model is not configured in Hubu"));
    }

    #[test]
    fn image_provider_config_rejects_unwired_external_adapter_before_payment() {
        let config = ImageProviderConfig {
            provider: "nano-banana".to_string(),
            model: "logo-v1".to_string(),
            merchant: "hubu-model-proxy".to_string(),
            api_key: Some("server-side-secret".to_string()),
            output_dir: std::env::temp_dir(),
            adapter_kind: ImageProviderAdapterKind::Unsupported("unconfigured".to_string()),
        };

        let resolved = config
            .resolve(None, None)
            .expect("configured provider/model should resolve");
        assert_eq!(resolved, ("nano-banana".to_string(), "logo-v1".to_string()));
        let error = match config.adapter() {
            Ok(_) => panic!("external provider should not fall back to the demo adapter"),
            Err(error) => error,
        };
        assert!(error
            .to_string()
            .contains("image provider adapter 'unconfigured' is not supported"));

        let demo_error = match (ImageProviderConfig {
            adapter_kind: ImageProviderAdapterKind::Demo,
            ..config
        }
        .adapter())
        {
            Ok(_) => panic!("demo adapter should not masquerade as an external provider"),
            Err(error) => error,
        };
        assert!(demo_error
            .to_string()
            .contains("demo image adapter can only be used with the hubu-demo provider"));
    }

    #[test]
    fn image_proxy_rejects_unwired_external_adapter_without_consuming_spend() {
        let path = std::env::temp_dir().join(format!(
            "hubu-api-unwired-image-provider-{}.sqlite",
            UserId::new()
        ));
        let mut state =
            ServerState::new_with_db_path(&path).expect("server state should initialize");
        init(
            json!({
                "display_name": "Alice Example",
                "email": "alice@example.com",
            })
            .to_string(),
            &state,
        )
        .expect("init should create an explicit user");

        let agent = register_agent(
            json!({
                "name": "logo-design-agent",
                "version": "v1",
            })
            .to_string(),
            &state,
        )
        .expect("agent should register under initialized user");

        add_policy(
            json!({
                "agent_id": agent.agent_id,
                "daily_limit_cents": 500,
            })
            .to_string(),
            &state,
        )
        .expect("policy should allow the logo budget");

        let logo_budget = create_budget(
            json!({
                "agent_id": agent.agent_id,
                "amount_cents": 500,
            })
            .to_string(),
            &state,
        )
        .expect("agent logo budget should be created");
        let logo_budget_id = logo_budget.budget.budget_id.clone();

        let authorization = authorize_spend(
            json!({
                "agent_id": agent.agent_id,
                "budget_id": logo_budget_id,
                "amount_cents": 500,
                "reason": "Generate Project Hubu logo",
                "merchant": "hubu-model-proxy",
            })
            .to_string(),
            &state,
        )
        .expect("spend should authorize");
        let auth_token_id = authorization
            .auth_token_id
            .expect("allowed spend should issue a token");

        state.image_provider = ImageProviderConfig {
            provider: "nano-banana".to_string(),
            model: "logo-v1".to_string(),
            merchant: "hubu-model-proxy".to_string(),
            api_key: Some("server-side-secret".to_string()),
            output_dir: std::env::temp_dir(),
            adapter_kind: ImageProviderAdapterKind::Unsupported("unconfigured".to_string()),
        };

        let error = generate_image(
            json!({
                "spend_auth_token_id": auth_token_id,
                "prompt": "Create a crisp logo for Project Hubu",
                "provider": "nano-banana",
                "model": "logo-v1",
            })
            .to_string(),
            &state,
        )
        .expect_err("unwired provider should fail before payment");
        assert!(error
            .to_string()
            .contains("image provider adapter 'unconfigured' is not supported"));

        let ledger = list_ledger(&state).expect("ledger should list");
        assert_eq!(ledger.transactions.len(), 0);
        let token_id = SpendAuthTokenId::from_str(&auth_token_id).expect("token id should parse");
        let token = state
            .spend
            .lock()
            .expect("spend manager lock should not be poisoned")
            .auth_token_record(&token_id)
            .expect("token should still exist");
        assert!(token.used_at.is_none());
        let hold = authorization
            .budget_hold
            .expect("authorized spend should freeze the logo budget");
        assert_eq!(hold.status, "frozen");
        assert_eq!(hold.frozen_amount_cents, 500);
        let logo_budget_internal_id =
            BudgetId::from_str(&logo_budget_id).expect("budget id should parse");
        let logo_budget = state
            .budgets
            .lock()
            .expect("budget manager lock should not be poisoned")
            .get_budget_by_id(&logo_budget_internal_id)
            .expect("logo budget should still exist");
        assert_eq!(logo_budget.balance.consumed_amount_cents, 0);
        assert_eq!(logo_budget.balance.frozen_amount_cents, 500);
        assert_eq!(logo_budget.balance.remaining_amount_cents, 0);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn image_proxy_releases_hold_without_payment_when_provider_generation_fails() {
        let path = std::env::temp_dir().join(format!(
            "hubu-api-image-provider-failure-{}.sqlite",
            UserId::new()
        ));
        let blocked_output_dir = std::env::temp_dir().join(format!(
            "hubu-api-image-provider-output-blocker-{}",
            UserId::new()
        ));
        std::fs::write(&blocked_output_dir, "not a directory")
            .expect("test blocker file should be writable");
        let mut state =
            ServerState::new_with_db_path(&path).expect("server state should initialize");
        state.image_provider.output_dir = blocked_output_dir.clone();
        init(
            json!({
                "display_name": "Alice Example",
                "email": "alice@example.com",
            })
            .to_string(),
            &state,
        )
        .expect("init should create an explicit user");

        let agent = register_agent(
            json!({
                "name": "logo-design-agent",
                "version": "v1",
            })
            .to_string(),
            &state,
        )
        .expect("agent should register under initialized user");

        add_policy(
            json!({
                "agent_id": agent.agent_id,
                "daily_limit_cents": 500,
            })
            .to_string(),
            &state,
        )
        .expect("policy should allow the logo budget");

        let logo_budget = create_budget(
            json!({
                "agent_id": agent.agent_id,
                "amount_cents": 500,
            })
            .to_string(),
            &state,
        )
        .expect("agent logo budget should be created");
        let logo_budget_id = logo_budget.budget.budget_id.clone();

        let authorization = authorize_spend(
            json!({
                "agent_id": agent.agent_id,
                "budget_id": logo_budget_id,
                "amount_cents": 500,
                "reason": "Generate Project Hubu logo",
                "merchant": "hubu-model-proxy",
            })
            .to_string(),
            &state,
        )
        .expect("spend should authorize");
        let auth_token_id = authorization
            .auth_token_id
            .expect("allowed spend should issue a token");

        let error = generate_image(
            json!({
                "spend_auth_token_id": auth_token_id,
                "prompt": "Create a crisp logo for Project Hubu",
            })
            .to_string(),
            &state,
        )
        .expect_err("provider generation should fail before payment");
        assert!(error
            .to_string()
            .contains("image provider generation failed"));

        let ledger = list_ledger(&state).expect("ledger should list");
        assert_eq!(ledger.transactions.len(), 0);
        let token_id = SpendAuthTokenId::from_str(&auth_token_id).expect("token id should parse");
        let token = state
            .spend
            .lock()
            .expect("spend manager lock should not be poisoned")
            .auth_token_record(&token_id)
            .expect("token should still exist");
        assert!(token.used_at.is_none());
        let logo_budget_internal_id =
            BudgetId::from_str(&logo_budget_id).expect("budget id should parse");
        let logo_budget = state
            .budgets
            .lock()
            .expect("budget manager lock should not be poisoned")
            .get_budget_by_id(&logo_budget_internal_id)
            .expect("logo budget should still exist");
        assert_eq!(logo_budget.balance.consumed_amount_cents, 0);
        assert_eq!(logo_budget.balance.frozen_amount_cents, 0);
        assert_eq!(logo_budget.balance.remaining_amount_cents, 500);
        std::fs::remove_file(blocked_output_dir).ok();
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn image_proxy_rejects_spend_authorized_for_other_merchants() {
        let path = std::env::temp_dir().join(format!(
            "hubu-api-image-scope-guard-{}.sqlite",
            UserId::new()
        ));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        init(
            json!({
                "display_name": "Alice Example",
                "email": "alice@example.com",
            })
            .to_string(),
            &state,
        )
        .expect("init should create an explicit user");

        let agent = register_agent(
            json!({
                "name": "logo-design-agent",
                "version": "v1",
            })
            .to_string(),
            &state,
        )
        .expect("agent should register under initialized user");

        add_policy(
            json!({
                "agent_id": agent.agent_id,
                "daily_limit_cents": 500,
            })
            .to_string(),
            &state,
        )
        .expect("policy should allow the small spend");

        create_budget(
            json!({
                "amount_cents": 500,
            })
            .to_string(),
            &state,
        )
        .expect("budget should be created for initialized user");

        let authorization = authorize_spend(
            json!({
                "agent_id": agent.agent_id,
                "amount_cents": 500,
                "reason": "Generate Project Hubu logo",
                "merchant": "other-model-proxy",
            })
            .to_string(),
            &state,
        )
        .expect("spend should authorize for a different merchant");
        let auth_token_id = authorization
            .auth_token_id
            .expect("allowed spend should issue a token");

        let error = generate_image(
            json!({
                "spend_auth_token_id": auth_token_id,
                "prompt": "Create a crisp logo for Project Hubu",
            })
            .to_string(),
            &state,
        )
        .expect_err("image proxy should reject unscoped merchant authorization");
        assert!(error
            .to_string()
            .contains("spend authorization is not scoped to the configured image proxy merchant"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn spend_rejects_conflicting_account_and_agent_anchors() {
        let path = std::env::temp_dir().join(format!(
            "hubu-api-conflicting-anchors-{}.sqlite",
            UserId::new()
        ));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        init(
            json!({
                "display_name": "Alice Example",
                "email": "alice@example.com",
            })
            .to_string(),
            &state,
        )
        .expect("init should create an explicit user");

        let agent = register_agent(
            json!({
                "name": "conflicting-anchor-agent",
                "version": "v1",
            })
            .to_string(),
            &state,
        )
        .expect("agent should register under initialized user");

        let error = spend(
            json!({
                "account_id": agent.account_id,
                "agent_id": agent.agent_id,
                "amount_cents": 2_500,
                "reason": "ambiguous anchored purchase",
            })
            .to_string(),
            &state,
        )
        .expect_err("spend should reject conflicting anchors");

        assert!(error
            .to_string()
            .contains("spend request must include exactly one of account_id or agent_id"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn yaml_policy_add_and_agent_list_use_registered_user() {
        let path = std::env::temp_dir().join(format!("hubu-api-policy-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        let user = init(
            json!({
                "display_name": "Alice Example",
                "email": "alice@example.com",
            })
            .to_string(),
            &state,
        )
        .expect("init should create an explicit user");

        let agent = register_agent(
            json!({
                "name": "yaml-policy-agent",
                "version": "v1",
            })
            .to_string(),
            &state,
        )
        .expect("agent should register");

        let agents = list_agents(&state).expect("agents should list");
        assert_eq!(agents.agents.len(), 1);
        assert_eq!(agents.agents[0].agent_id, agent.agent_id);
        assert_eq!(agents.agents[0].display_name, "yaml-policy-agent");

        let policy = add_policy(
            json!({
                "agent_id": agent.agent_id,
                "policy_yaml": r#"
id: yaml_demo_policy
version: demo-1
owner_user_id: 00000000-0000-4000-8000-000000000000
default_effect: needs_approval
rules:
  - id: allow_small_yaml_spend
    effect: allow
    reason: yaml policy allowed this spend
    when:
      op: lte
      field: amount
      value:
        money_cents: 5000
"#,
            })
            .to_string(),
            &state,
        )
        .expect("yaml policy should be added");
        assert_eq!(policy.policy_id, "yaml_demo_policy");
        assert_eq!(policy.policy_version, "demo-1");

        create_budget(
            json!({
                "amount_cents": 10_000,
            })
            .to_string(),
            &state,
        )
        .expect("budget should be created");

        let spend = spend(
            json!({
                "agent_id": agents.agents[0].agent_id,
                "amount_cents": 2_500,
                "reason": "yaml-backed policy spend",
            })
            .to_string(),
            &state,
        )
        .expect("spend should use yaml policy");
        assert_eq!(spend.decision, "allow");
        assert_eq!(
            spend
                .payment
                .expect("allowed spend should pay")
                .owner_user_id,
            user.user_id
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn failed_mock_payment_releases_reserved_budget() {
        let path = std::env::temp_dir().join(format!("hubu-api-failed-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        let user = init(
            json!({
                "display_name": "Alice Example",
                "email": "alice@example.com",
            })
            .to_string(),
            &state,
        )
        .expect("init should create an explicit user");

        let agent = register_agent(
            json!({
                "name": "failed-payment-agent",
                "version": "v1",
            })
            .to_string(),
            &state,
        )
        .expect("agent should register under initialized user");

        add_policy(
            json!({
                "agent_id": agent.agent_id,
                "daily_limit_cents": 5_000,
            })
            .to_string(),
            &state,
        )
        .expect("policy should be added under initialized user");

        create_budget(
            json!({
                "amount_cents": 10_000,
            })
            .to_string(),
            &state,
        )
        .expect("budget should be created for initialized user");

        let spend = spend(
            json!({
                "agent_id": agent.agent_id,
                "amount_cents": 1_500,
                "reason": "failed test purchase",
                "merchant": "fail",
            })
            .to_string(),
            &state,
        )
        .expect("allowed spend should submit a failed mock payment");

        let payment = spend.payment.expect("allowed spend should attempt payment");
        assert_eq!(payment.status, "failed");
        assert_eq!(payment.owner_user_id, user.user_id);
        assert!(payment.failure_reason.is_some());

        let budget_hold = spend
            .budget_hold
            .expect("allowed spend should reserve and release budget");
        assert_eq!(budget_hold.status, "released");
        assert_eq!(budget_hold.consumed_amount_cents, 0);
        assert_eq!(budget_hold.frozen_amount_cents, 0);
        assert_eq!(budget_hold.remaining_amount_cents, 10_000);

        let ledger = list_ledger(&state).expect("ledger should list");
        assert!(ledger.transactions.is_empty());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn over_budget_spend_returns_structured_denial_response() {
        let path =
            std::env::temp_dir().join(format!("hubu-api-over-budget-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        init(
            json!({
                "display_name": "Alice Example",
                "email": "alice@example.com",
            })
            .to_string(),
            &state,
        )
        .expect("init should create an explicit user");

        let agent = register_agent(
            json!({
                "name": "over-budget-agent",
                "version": "v1",
            })
            .to_string(),
            &state,
        )
        .expect("agent should register under initialized user");

        add_policy(
            json!({
                "agent_id": agent.agent_id,
                "daily_limit_cents": 5_000,
            })
            .to_string(),
            &state,
        )
        .expect("policy should allow the attempted spend");

        create_budget(
            json!({
                "amount_cents": 1_000,
            })
            .to_string(),
            &state,
        )
        .expect("budget should be created for initialized user");

        let response = route(
            HttpRequest {
                method: "POST".to_string(),
                path: "/spend".to_string(),
                body: json!({
                    "agent_id": agent.agent_id,
                    "amount_cents": 2_500,
                    "reason": "over budget purchase",
                    "merchant": "Acme Cafe",
                })
                .to_string(),
            },
            &state,
        );

        assert_eq!(response.status, 200);
        assert_eq!(response.body["decision"], "deny");
        assert_eq!(response.body["auth_token_id"], Value::Null);
        assert_eq!(response.body["budget_hold"], Value::Null);
        assert_eq!(response.body["payment"], Value::Null);
        assert!(response.body["reasons"]
            .as_array()
            .expect("reasons should be an array")
            .iter()
            .any(|reason| reason == "budget does not have enough remaining balance"));

        let budgets = list_budgets(&state).expect("budgets should list");
        assert_eq!(budgets.budgets[0].remaining_amount_cents, 1_000);
        assert_eq!(budgets.budgets[0].frozen_amount_cents, 0);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn initialized_user_remains_registration_owner_after_restart() {
        let path = std::env::temp_dir().join(format!("hubu-api-restart-{}.sqlite", UserId::new()));
        let (user, first_agent_id) = {
            let state =
                ServerState::new_with_db_path(&path).expect("server state should initialize");
            let user = init(
                json!({
                    "display_name": "Alice Example",
                    "email": "alice@example.com",
                })
                .to_string(),
                &state,
            )
            .expect("init should create an explicit user");

            let agent = register_agent(
                json!({
                    "name": "settlement-agent",
                    "version": "v1",
                })
                .to_string(),
                &state,
            )
            .expect("agent should register under initialized user");
            assert_eq!(agent.user_id, user.user_id);
            (user, agent.agent_id)
        };

        let restarted =
            ServerState::new_with_db_path(&path).expect("server state should reload from storage");
        let resumed_agent = register_agent(
            json!({
                "name": "settlement-agent",
                "version": "v1",
            })
            .to_string(),
            &restarted,
        )
        .expect("agent should still register under initialized user after restart");

        assert_eq!(resumed_agent.user_id, user.user_id);
        assert_eq!(resumed_agent.agent_id, first_agent_id);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn governance_and_payment_audit_state_survive_restart() {
        let path = std::env::temp_dir().join(format!(
            "hubu-api-governance-restart-{}.sqlite",
            UserId::new()
        ));
        let (user_id, agent_id) = {
            let state =
                ServerState::new_with_db_path(&path).expect("server state should initialize");
            let user = init(
                json!({
                    "display_name": "Alice Example",
                    "email": "alice@example.com",
                })
                .to_string(),
                &state,
            )
            .expect("init should create an explicit user");
            let agent = register_agent(
                json!({
                    "name": "durable-agent",
                    "version": "v1",
                })
                .to_string(),
                &state,
            )
            .expect("agent should register");
            add_policy(
                json!({
                    "agent_id": agent.agent_id,
                    "daily_limit_cents": 5_000,
                })
                .to_string(),
                &state,
            )
            .expect("policy should be added");
            create_budget(
                json!({
                    "amount_cents": 10_000,
                })
                .to_string(),
                &state,
            )
            .expect("budget should be created");
            spend(
                json!({
                    "agent_id": agent.agent_id,
                    "amount_cents": 2_500,
                    "reason": "restart audit purchase",
                    "merchant": "Acme Cafe",
                })
                .to_string(),
                &state,
            )
            .expect("spend should be approved and paid");
            (user.user_id, agent.agent_id)
        };

        let restarted =
            ServerState::new_with_db_path(&path).expect("server state should reload from storage");
        let budgets = list_budgets(&restarted).expect("budgets should reload");
        assert_eq!(budgets.budgets.len(), 1);
        assert_eq!(budgets.budgets[0].consumed_amount_cents, 2_500);
        assert_eq!(budgets.budgets[0].remaining_amount_cents, 7_500);

        let ledger = list_ledger(&restarted).expect("ledger should reload");
        assert_eq!(ledger.transactions.len(), 1);
        assert_eq!(ledger.transactions[0].owner_user_id, user_id);

        let attempts = restarted
            .payment_attempts
            .lock()
            .expect("payment attempt store lock should not be poisoned")
            .list_payment_attempts()
            .expect("payment attempts should reload");
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].status, PaymentStatus::Succeeded);

        let resumed_spend = spend(
            json!({
                "agent_id": agent_id,
                "amount_cents": 1_000,
                "reason": "post-restart policy spend",
                "merchant": "Acme Cafe",
            })
            .to_string(),
            &restarted,
        )
        .expect("restarted server should still have the policy assignment");
        assert_eq!(resumed_spend.decision, "allow");
        std::fs::remove_file(path).ok();
    }
}
