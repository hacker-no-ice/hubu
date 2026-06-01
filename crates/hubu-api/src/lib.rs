use std::{
    collections::HashMap,
    env,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
};

use anyhow::{anyhow, Context, Result};
use hubu_common::{
    actor::{OwnerRef, OwnerType},
    ids::{AgentId, SpendAuthTokenId},
    models::identity::{
        AgentType, CodeReference, ModelIdentity, RuntimeEnvironment, RuntimeIdentity,
    },
    money::Currency,
};
use hubu_core::{
    policy::{
        condition::{Condition, Field, PolicyValue},
        model::{Effect, Policy, Rule},
    },
    registration::{RegisterAgentRequest, RegistrationManager},
    spend::{
        model::{SpendPaymentValidationRequest, SpendRequest},
        SpendManager,
    },
};
use hubu_wallet::{
    LedgerTransaction, MockPaymentRail, PaymentDestination, PaymentError, PaymentManager,
    PaymentRailKind, PaymentRequest, PaymentStatus, SpendAuthorizationValidator, SqliteLedger,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const DEFAULT_BIND_ADDR: &str = "127.0.0.1:8787";

type DemoPaymentManager = PaymentManager<MockPaymentRail, SharedSpendAuthorizer>;

pub fn run_server_from_env() -> Result<()> {
    let bind_addr = env::args()
        .nth(1)
        .or_else(|| env::var("HUBU_BIND_ADDR").ok())
        .unwrap_or_else(|| DEFAULT_BIND_ADDR.to_string());
    run_server(&bind_addr)
}

pub fn run_server(bind_addr: &str) -> Result<()> {
    let listener = TcpListener::bind(bind_addr)
        .with_context(|| format!("bind Hubu demo server to {bind_addr}"))?;
    let state = ServerState::new()?;

    println!("Hubu demo server listening on http://{bind_addr}");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(error) = handle_connection(stream, &state) {
                    eprintln!("request error: {error:#}");
                }
            }
            Err(error) => eprintln!("connection error: {error:#}"),
        }
    }

    Ok(())
}

struct ServerState {
    registration: Mutex<RegistrationManager>,
    spend: Arc<Mutex<SpendManager>>,
    policies: Mutex<HashMap<AgentId, Policy>>,
    payments: Mutex<DemoPaymentManager>,
}

impl ServerState {
    fn new() -> Result<Self> {
        let spend = Arc::new(Mutex::new(SpendManager::new()));
        let authorizer = SharedSpendAuthorizer {
            spend: Arc::clone(&spend),
        };
        let payments = PaymentManager::new(
            MockPaymentRail,
            authorizer,
            SqliteLedger::in_memory().context("initialize in-memory ledger")?,
        )
        .context("initialize payment manager")?;

        Ok(Self {
            registration: Mutex::new(RegistrationManager::new()),
            spend,
            policies: Mutex::new(HashMap::new()),
            payments: Mutex::new(payments),
        })
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
                agent_id: request.agent_id.clone(),
                amount_cents: request.amount_cents,
                currency: request.currency,
                merchant: request.merchant.clone(),
                task_id: request.task_id.clone(),
            })
            .map(|_| hubu_wallet::ValidatedSpendAuthorization {
                spend_auth_token_id: request.spend_auth_token_id.clone(),
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

#[derive(Debug, Deserialize)]
struct RegisterAgentHttpRequest {
    name: String,
    version: String,
}

#[derive(Debug, Serialize)]
struct RegisterAgentHttpResponse {
    agent_id: String,
    agent_pub_id: String,
    version_id: String,
    account_id: String,
    session_id: String,
}

#[derive(Debug, Deserialize)]
struct AddPolicyHttpRequest {
    agent_id: String,
    daily_limit_cents: i64,
}

#[derive(Debug, Serialize)]
struct AddPolicyHttpResponse {
    agent_id: String,
    policy_id: String,
    daily_limit_cents: i64,
}

#[derive(Debug, Deserialize)]
struct SpendHttpRequest {
    agent_id: String,
    amount_cents: i64,
    reason: String,
    merchant: Option<String>,
}

#[derive(Debug, Serialize)]
struct SpendHttpResponse {
    decision_id: String,
    decision: String,
    reasons: Vec<String>,
    auth_token_id: Option<String>,
    payment: Option<PaymentHttpResponse>,
}

#[derive(Debug, Serialize)]
struct PaymentHttpResponse {
    payment_id: String,
    status: String,
    ledger_transaction_id: Option<String>,
    rail_reference: Option<String>,
}

#[derive(Debug, Serialize)]
struct LedgerHttpResponse {
    transactions: Vec<LedgerTransactionHttpResponse>,
}

#[derive(Debug, Serialize)]
struct LedgerTransactionHttpResponse {
    id: String,
    external_ref: Option<String>,
    description: String,
    created_at: String,
    entries: Vec<LedgerEntryHttpResponse>,
}

#[derive(Debug, Serialize)]
struct LedgerEntryHttpResponse {
    id: String,
    account_id: String,
    direction: String,
    amount_cents: i64,
    currency: String,
}

fn handle_connection(mut stream: TcpStream, state: &ServerState) -> Result<()> {
    let mut raw = String::new();
    stream.read_to_string(&mut raw)?;
    if raw.is_empty() {
        return Ok(());
    }

    let request = parse_request(&raw)?;
    let response = route(request, state);
    write_response(&mut stream, response)
}

fn route(request: HttpRequest, state: &ServerState) -> HttpResponse {
    let result = match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") => Ok(json!({ "status": "ok" })),
        ("POST", "/agents/register") => register_agent(request.body, state).map(to_json),
        ("POST", "/policies") => add_policy(request.body, state).map(to_json),
        ("POST", "/spend") => spend(request.body, state).map(to_json),
        ("GET", "/ledger") => list_ledger(state).map(to_json),
        _ => Err(anyhow!("no route for {} {}", request.method, request.path)),
    };

    match result {
        Ok(body) => HttpResponse { status: 200, body },
        Err(error) => HttpResponse {
            status: 400,
            body: json!({ "error": error.to_string() }),
        },
    }
}

fn register_agent(body: String, state: &ServerState) -> Result<RegisterAgentHttpResponse> {
    let request: RegisterAgentHttpRequest = serde_json::from_str(&body)?;
    let registration_request = RegisterAgentRequest {
        display_name: request.name.clone(),
        description: Some("Registered through the Hubu demo CLI".to_string()),
        owner: OwnerRef {
            owner_type: OwnerType::Human,
            owner_id: "demo-user".to_string(),
        },
        agent_type: AgentType::AutonomousAgent,
        identity_fingerprint: format!("demo:agent:{}", request.name),
        version_fingerprint: format!("demo:agent:{}:{}", request.name, request.version),
        code_ref: Some(CodeReference {
            repository_url: None,
            commit_sha: Some(request.version.clone()),
        }),
        model: Some(ModelIdentity {
            provider: "demo".to_string(),
            model: request.name.clone(),
            version: Some(request.version),
        }),
        runtime: Some(RuntimeIdentity {
            runtime_provider: "hubu-cli".to_string(),
            environment: RuntimeEnvironment::Development,
        }),
        mcp_client_name: Some("hubu-cli".to_string()),
        mcp_client_version: env!("CARGO_PKG_VERSION").to_string().into(),
    };

    let response = state
        .registration
        .lock()
        .map_err(|_| anyhow!("registration manager lock poisoned"))?
        .register_agent(registration_request)?;

    Ok(RegisterAgentHttpResponse {
        agent_id: response.agent.pub_id.clone(),
        agent_pub_id: response.agent.pub_id,
        version_id: response.version.pub_id,
        account_id: response.account.pub_id,
        session_id: response.session.pub_id,
    })
}

fn add_policy(body: String, state: &ServerState) -> Result<AddPolicyHttpResponse> {
    let request: AddPolicyHttpRequest = serde_json::from_str(&body)?;
    if request.daily_limit_cents <= 0 {
        return Err(anyhow!("daily limit must be positive"));
    }

    let agent_pub_id = request.agent_id;
    let agent_id = resolve_agent_id(&agent_pub_id, state)?;
    let policy_id = format!("demo_policy_{agent_pub_id}");
    let policy = Policy {
        id: policy_id.clone(),
        version: "demo-1".to_string(),
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
                    value: PolicyValue::MoneyCents(request.daily_limit_cents),
                },
                reason: format!(
                    "amount is at or below the configured demo limit of {} cents",
                    request.daily_limit_cents
                ),
            },
        ],
    };

    state
        .policies
        .lock()
        .map_err(|_| anyhow!("policy store lock poisoned"))?
        .insert(agent_id, policy);

    Ok(AddPolicyHttpResponse {
        agent_id: agent_pub_id,
        policy_id,
        daily_limit_cents: request.daily_limit_cents,
    })
}

fn spend(body: String, state: &ServerState) -> Result<SpendHttpResponse> {
    let request: SpendHttpRequest = serde_json::from_str(&body)?;
    if request.amount_cents <= 0 {
        return Err(anyhow!("spend amount must be positive"));
    }

    let agent_pub_id = request.agent_id;
    let agent_id = resolve_agent_id(&agent_pub_id, state)?;
    let policy = state
        .policies
        .lock()
        .map_err(|_| anyhow!("policy store lock poisoned"))?
        .get(&agent_id)
        .cloned()
        .ok_or_else(|| anyhow!("no policy found for agent {agent_pub_id}"))?;

    let spend_request = SpendRequest {
        amount_cents: request.amount_cents,
        currency: Currency::Usd,
        agent_id: agent_id.clone(),
        merchant: request.merchant.clone(),
        category: None,
        task_id: Some(request.reason.clone()),
    };

    let evaluation = state
        .spend
        .lock()
        .map_err(|_| anyhow!("spend manager lock poisoned"))?
        .evaluate_spend(spend_request.clone(), &policy)?;

    let auth_token_id = evaluation
        .auth_token
        .as_ref()
        .map(|token| token.id.to_string());
    let payment = if let Some(token) = evaluation.auth_token {
        let payment = state
            .payments
            .lock()
            .map_err(|_| anyhow!("payment manager lock poisoned"))?
            .submit_payment(PaymentRequest {
                idempotency_key: format!("{}:{}", evaluation.decision_id, request.reason),
                spend_auth_token_id: token.id,
                agent_id,
                amount_cents: request.amount_cents,
                currency: Currency::Usd,
                merchant: request.merchant,
                task_id: Some(request.reason),
                rail: PaymentRailKind::FiatMock,
                destination: PaymentDestination::FiatAccount {
                    account_ref: "demo-merchant-account".to_string(),
                },
                memo: Some("Hubu demo payment".to_string()),
            })?;

        Some(PaymentHttpResponse {
            payment_id: payment.payment_id.to_string(),
            status: match payment.status {
                PaymentStatus::Succeeded => "succeeded",
                PaymentStatus::Failed => "failed",
            }
            .to_string(),
            ledger_transaction_id: payment.ledger_transaction_id.map(|id| id.to_string()),
            rail_reference: payment.rail_reference,
        })
    } else {
        None
    };

    Ok(SpendHttpResponse {
        decision_id: evaluation.decision_id.to_string(),
        decision: effect_name(evaluation.evaluation.decision).to_string(),
        reasons: evaluation.evaluation.reasons,
        auth_token_id,
        payment,
    })
}

fn resolve_agent_id(agent_pub_id: &str, state: &ServerState) -> Result<AgentId> {
    state
        .registration
        .lock()
        .map_err(|_| anyhow!("registration manager lock poisoned"))?
        .agent_id_for_pub_id(agent_pub_id)
        .ok_or_else(|| anyhow!("unknown public agent id {agent_pub_id}"))
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
        .map(|transaction| ledger_transaction_response(payments.ledger(), transaction))
        .collect::<Result<Vec<_>>>()?;

    Ok(LedgerHttpResponse { transactions })
}

fn ledger_transaction_response(
    ledger: &SqliteLedger,
    transaction: LedgerTransaction,
) -> Result<LedgerTransactionHttpResponse> {
    let entries = ledger
        .entries_for_transaction(&transaction.id)?
        .into_iter()
        .map(|entry| LedgerEntryHttpResponse {
            id: entry.id.to_string(),
            account_id: entry.account_id.to_string(),
            direction: match entry.direction {
                hubu_wallet::LedgerDirection::Debit => "debit",
                hubu_wallet::LedgerDirection::Credit => "credit",
            }
            .to_string(),
            amount_cents: entry.amount_cents,
            currency: entry.currency.to_string(),
        })
        .collect();

    Ok(LedgerTransactionHttpResponse {
        id: transaction.id.to_string(),
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
