use std::{
    collections::HashMap,
    env,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    sync::{Arc, Mutex},
};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use hubu_common::{
    ids::{AgentId, BudgetId, SpendAuthTokenId, UserId},
    models::identity::{
        AgentType, CodeReference, ModelIdentity, RuntimeEnvironment, RuntimeIdentity,
    },
    models::UserContext,
    money::Currency,
    time::TimePeriod,
};
use hubu_core::{
    budget::{
        BudgetManager, BudgetRecurrence, BudgetScope, BudgetWithBalance, CreateBudgetSeriesRequest,
        CreateSingleBudgetRequest, ReleaseBudgetResponse, ReserveBudgetRequest,
        SettleBudgetResponse,
    },
    policy::{
        condition::{Condition, Field, PolicyValue},
        model::{Effect, Policy, Rule},
    },
    registration::{RegisterAgentRequest, RegistrationManager},
    spend::{
        model::{SpendPaymentValidationRequest, SpendRequest},
        SpendManager,
    },
    user::{CreateUserRequest, UserManager},
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
    users: Mutex<UserManager>,
    registration: Mutex<RegistrationManager>,
    spend: Arc<Mutex<SpendManager>>,
    budgets: Mutex<BudgetManager>,
    policies: Mutex<HashMap<(UserId, AgentId), Policy>>,
    payments: Mutex<DemoPaymentManager>,
}

impl ServerState {
    fn new() -> Result<Self> {
        Self::new_with_db_path(
            env::var("HUBU_DB_PATH").unwrap_or_else(|_| "hubu.sqlite3".to_string()),
        )
    }

    fn new_with_db_path(path: impl AsRef<Path>) -> Result<Self> {
        let mut users = UserManager::open(path.as_ref()).context("initialize user store")?;
        let default_user = users.ensure_default_user()?;
        let spend = Arc::new(Mutex::new(SpendManager::new()));
        let authorizer = SharedSpendAuthorizer {
            spend: Arc::clone(&spend),
        };
        let payments = PaymentManager::new(
            default_user.id,
            MockPaymentRail,
            authorizer,
            SqliteLedger::in_memory().context("initialize in-memory ledger")?,
        )
        .context("initialize payment manager")?;

        Ok(Self {
            users: Mutex::new(users),
            registration: Mutex::new(
                RegistrationManager::open(path).context("initialize agent registration store")?,
            ),
            spend,
            budgets: Mutex::new(BudgetManager::new()),
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
                owner_user_id: request.owner_user_id.clone(),
                agent_id: request.agent_id.clone(),
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

#[derive(Debug, Deserialize)]
struct RegisterAgentHttpRequest {
    name: String,
    version: String,
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
    daily_limit_cents: i64,
}

#[derive(Debug, Serialize)]
struct AddPolicyHttpResponse {
    agent_id: String,
    policy_id: String,
    daily_limit_cents: i64,
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
        ("POST", "/init") => init(request.body, state).map(to_json),
        ("POST", "/agents/register") => register_agent(request.body, state).map(to_json),
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
        Err(error) => HttpResponse {
            status: 400,
            body: json!({ "error": error.to_string() }),
        },
    }
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

fn register_agent(body: String, state: &ServerState) -> Result<RegisterAgentHttpResponse> {
    let request: RegisterAgentHttpRequest = serde_json::from_str(&body)?;
    let user = default_user_context(state)?;
    let registration_request = RegisterAgentRequest {
        display_name: request.name.clone(),
        description: Some("Registered through the Hubu demo CLI".to_string()),
        owner_user_id: user.user_id.clone(),
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
        user_id: default_user_pub_id(state)?,
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
    let user = default_user_context(state)?;
    let agent_id = resolve_agent_id_for_user(&agent_pub_id, &user, state)?;
    let policy_id = format!("demo_policy_{agent_pub_id}");
    let policy = Policy {
        id: policy_id.clone(),
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
        .insert((user.user_id, agent_id), policy);

    Ok(AddPolicyHttpResponse {
        agent_id: agent_pub_id,
        policy_id,
        daily_limit_cents: request.daily_limit_cents,
    })
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
            scope: BudgetScope::User(user.user_id),
            amount_limit_cents: request.amount_cents,
            currency: Currency::Usd,
            period,
        })?;

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
            scope: BudgetScope::User(user.user_id),
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

    let agent_pub_id = request.agent_id;
    let user = default_user_context(state)?;
    let agent_id = resolve_agent_id_for_user(&agent_pub_id, &user, state)?;
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
        merchant: request.merchant.clone(),
        category: None,
        task_id: Some(request.reason.clone()),
    };

    let evaluation = state
        .spend
        .lock()
        .map_err(|_| anyhow!("spend manager lock poisoned"))?
        .evaluate_spend(&user, spend_request.clone(), &policy)?;

    let auth_token_id = evaluation
        .auth_token
        .as_ref()
        .map(|token| token.id.to_string());
    let owner = owner_metadata_for_user_id(&user.user_id, state)?;
    let (budget_hold, payment) = if let Some(token) = evaluation.auth_token {
        let budget_id = active_budget_id_for_user(&user.user_id, state)?;
        let reservation = state
            .budgets
            .lock()
            .map_err(|_| anyhow!("budget manager lock poisoned"))?
            .reserve_budget(ReserveBudgetRequest {
                budget_id,
                spend_decision_id: evaluation.decision_id.clone(),
                amount_cents: request.amount_cents,
                currency: Currency::Usd,
                expires_at: token.expires_at,
            })?;

        let payment_request = PaymentRequest {
            idempotency_key: format!("{}:{}", evaluation.decision_id, request.reason),
            spend_auth_token_id: token.id,
            owner_user_id: user.user_id.clone(),
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
        };

        let payment_result = state
            .payments
            .lock()
            .map_err(|_| anyhow!("payment manager lock poisoned"))?
            .submit_payment(payment_request);

        match payment_result {
            Ok(payment) => {
                let hold_update = if payment.status == PaymentStatus::Succeeded {
                    BudgetHoldUpdate::Settled(
                        state
                            .budgets
                            .lock()
                            .map_err(|_| anyhow!("budget manager lock poisoned"))?
                            .settle_budget(&reservation.hold.id)?,
                    )
                } else {
                    BudgetHoldUpdate::Released(
                        state
                            .budgets
                            .lock()
                            .map_err(|_| anyhow!("budget manager lock poisoned"))?
                            .release_budget(&reservation.hold.id)?,
                    )
                };

                (
                    Some(budget_hold_response(hold_update)),
                    Some(PaymentHttpResponse {
                        payment_id: payment.payment_id.to_string(),
                        owner_user_id: owner.pub_id,
                        owner_user_name: owner.display_name,
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
                state
                    .budgets
                    .lock()
                    .map_err(|_| anyhow!("budget manager lock poisoned"))?
                    .release_budget(&reservation.hold.id)?;
                return Err(error.into());
            }
        }
    } else {
        (None, None)
    };

    Ok(SpendHttpResponse {
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

fn default_user_pub_id(state: &ServerState) -> Result<String> {
    let mut users = state
        .users
        .lock()
        .map_err(|_| anyhow!("user manager lock poisoned"))?;
    Ok(users.ensure_default_user()?.pub_id)
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
}
