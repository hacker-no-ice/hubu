use std::{
    collections::{BTreeMap, HashMap},
    env,
    fmt::Write as _,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
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
        ReserveBudgetRequest, SettleBudgetResponse,
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
        };
        log_event(
            "info",
            "server_state_initialized",
            json!({
                "db_path": db_path,
                "default_user_id": default_user.id.to_string(),
                "default_user_pub_id": default_user.pub_id,
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
        ("POST", "/spend") => spend(request.body, state).map(to_json),
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
            scope: BudgetScope::User(user.user_id.clone()),
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
    let budgets = state
        .budgets
        .lock()
        .map_err(|_| anyhow!("budget manager lock poisoned"))?
        .get_budgets_by_user_id(&user.user_id)
        .into_iter()
        .map(budget_response)
        .collect();

    Ok(ListBudgetsHttpResponse { budgets })
}

fn spend(body: String, state: &ServerState) -> Result<SpendHttpResponse> {
    let request: SpendHttpRequest = serde_json::from_str(&body)?;
    if request.amount_cents <= 0 {
        return Err(anyhow!("spend amount must be positive"));
    }

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
    let owner = owner_metadata_for_user_id(&user.user_id, state)?;
    let (budget_hold, payment) = if let Some(token) = evaluation.auth_token {
        let budget_id = active_budget_id_for_user(&user.user_id, state)?;
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
                    return Ok(SpendHttpResponse {
                        account_id: account_pub_id,
                        agent_id: agent_pub_id,
                        decision_id: evaluation.decision_id.to_string(),
                        decision: "deny".to_string(),
                        reasons: vec!["budget does not have enough remaining balance".to_string()],
                        auth_token_id: None,
                        budget_hold: None,
                        payment: None,
                    });
                }
                Err(error) => return Err(error.into()),
            };
            drop(budgets);
            state
                .governance
                .lock()
                .map_err(|_| anyhow!("governance store lock poisoned"))?
                .save_budget_hold(&reservation.hold, &reservation.balance)?;
            if let Some(token_record) = &token_record {
                state
                    .governance
                    .lock()
                    .map_err(|_| anyhow!("governance store lock poisoned"))?
                    .save_spend_auth_token(token_record)?;
            }
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

        let payment_request = PaymentRequest {
            idempotency_key: format!("{}:{}", evaluation.decision_id, request.reason),
            spend_auth_token_id: token.id,
            owner_user_id: user.user_id.clone(),
            agent_id,
            agent_account_id: account.id.clone(),
            amount_cents: request.amount_cents,
            currency: Currency::Usd,
            merchant: request.merchant,
            task_id: Some(request.reason),
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
                        "decision_id": evaluation.decision_id.to_string(),
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
                        account_id: account_pub_id.clone(),
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
                        "decision_id": evaluation.decision_id.to_string(),
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
        account_id: account_pub_id,
        agent_id: agent_pub_id,
        decision_id: evaluation.decision_id.to_string(),
        decision: effect_name(evaluation.evaluation.decision).to_string(),
        reasons: evaluation.evaluation.reasons,
        auth_token_id,
        budget_hold,
        payment,
    })
}

enum BudgetHoldUpdate {
    Settled(SettleBudgetResponse),
    Released(ReleaseBudgetResponse),
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
