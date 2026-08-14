use std::{
    collections::{BTreeMap, HashMap},
    env,
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex},
    time::{Duration as StdDuration, Instant},
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration, Months, Utc};
use hubu_common::{
    build::{build_info, EXECUTOR_CONTRACT},
    ids::{
        AgentId, AgentSessionId, BudgetId, SpendAuthTokenId, SpendExecutorClaimId,
        SpendingTargetId, UserId,
    },
    models::account::{AccountStatus, AgentAccount},
    models::identity::{
        AgentType, CodeReference, ModelIdentity, RuntimeEnvironment, RuntimeIdentity,
    },
    models::{User, UserContext},
    money::Currency,
    time::TimePeriod,
};
use hubu_core::{
    app::{
        ApprovedSpendAuthorization, AuthorizeSpendRequest, BudgetHoldUpdate,
        ClaimExecutorSpendRequest, ExecutorClaimReconciliationOutcome, ExecutorClaimService,
        ExecutorClaimState, FailedPaymentHoldPolicy, FinalizeExecutorClaimRequest,
        ReconcileExecutorClaimRequest, RejectedSpendAuthorization, SettleExecutorClaimRequest,
        SpendApprovalError, SpendApprovalService, SpendAuthorizationOutcome, SpendPaymentSpec,
    },
    budget::{
        BudgetHold, BudgetHoldStatus, BudgetManager, BudgetRecurrence, BudgetStatus,
        BudgetWithBalance, CreateBudgetSeriesRequest, CreateSingleBudgetRequest,
        ReserveBudgetResponse,
    },
    persistence::{
        BudgetRepository, PolicyAssignmentScope, PolicyRepository, SpendRepository,
        SpendingTargetRepository, SqliteGovernanceRepository,
    },
    policy::{
        condition::{Condition, Field, PolicyValue},
        engine::validate_policy,
        error::PolicyLoadError,
        model::{Effect, Policy, Rule},
    },
    registration::{AgentWithAccount, RegisterAgentRequest, RegistrationManager},
    spend::{
        SpendExecutorClaimRecord, SpendExecutorClaimStatus, SpendExecutorPriceModelSnapshot,
        SpendExecutorSettlementReceipt, SpendManager, SpendPaymentValidationRequest,
        SpendTimingConfig,
    },
    spending_target::{
        periods_overlap, CreateSpendingTargetRequest, SpendingTarget, SpendingTargetManager,
        SpendingTargetStatus,
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
const AUTH_TOKEN_ENV: &str = "HUBU_AUTH_TOKEN";
const AUTH_TOKEN_FILE_ENV: &str = "HUBU_AUTH_TOKEN_FILE";
const DEFAULT_AUTH_TOKEN_FILE: &str = "hubu.auth-token";
const RECONCILIATION_TOKEN_ENV: &str = "HUBU_RECONCILIATION_TOKEN";
const RECONCILIATION_TOKEN_FILE_ENV: &str = "HUBU_RECONCILIATION_TOKEN_FILE";
const DEFAULT_RECONCILIATION_TOKEN_FILE: &str = "hubu.reconciliation-token";
const RECONCILIATION_CAPABILITY_HEADER: &str = "x-hubu-reconciliation-capability";
const SPEND_TIMING_CONFIG_ENV: &str = "HUBU_SPEND_TIMING_CONFIG";
const HTTP_READ_TIMEOUT: StdDuration = StdDuration::from_secs(5);
const MAX_HTTP_HEADER_BYTES: usize = 64 * 1024;
const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;
#[cfg(test)]
const TEST_AUTH_TOKEN: &str = "test-local-auth-token";
#[cfg(test)]
const TEST_RECONCILIATION_TOKEN: &str = "test-human-reconciliation-token";
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

type LocalPaymentManager = PaymentManager<MockPaymentRail, SharedSpendAuthorizer>;

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
    let listener =
        TcpListener::bind(bind_addr).with_context(|| format!("bind Hubu server to {bind_addr}"))?;
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
    auth: LocalAuth,
    users: Mutex<UserManager>,
    registration: Mutex<RegistrationManager>,
    spend: Arc<Mutex<SpendManager>>,
    budgets: Mutex<BudgetManager>,
    spending_targets: Mutex<SpendingTargetManager>,
    policies: Mutex<HashMap<(UserId, PolicyAssignmentScope), Policy>>,
    governance: Mutex<SqliteGovernanceRepository>,
    payment_attempts: Mutex<SqlitePaymentAttemptRepository>,
    payments: Mutex<LocalPaymentManager>,
    spend_timing: SpendTimingConfig,
}

impl ServerState {
    fn new() -> Result<Self> {
        Self::new_with_db_path(
            env::var("HUBU_DB_PATH").unwrap_or_else(|_| "hubu.sqlite3".to_string()),
        )
    }

    fn new_with_db_path(path: impl AsRef<Path>) -> Result<Self> {
        Self::new_with_db_path_and_spend_timing(path, load_spend_timing_config()?)
    }

    fn new_with_db_path_and_spend_timing(
        path: impl AsRef<Path>,
        spend_timing: SpendTimingConfig,
    ) -> Result<Self> {
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
        let auth = LocalAuth::new(default_user.id.clone()).context("initialize local API auth")?;
        log_event(
            "info",
            "local_api_auth_configured",
            json!({
                "source": auth.source(),
                "reconciliation_source": auth.reconciliation_source(),
                "owner_user_id": default_user.id.to_string(),
                "owner_user_pub_id": default_user.pub_id,
            }),
        );
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
                    (assignment.owner_user_id, assignment.scope),
                    assignment.policy,
                )
            })
            .collect();
        let spend = Arc::new(Mutex::new(SpendManager::from_records_with_claims(
            governance
                .load_spend_decisions()
                .context("load spend decisions")?,
            governance
                .load_spend_auth_tokens()
                .context("load spend auth tokens")?,
            governance
                .load_executor_claims()
                .context("load executor spend claims")?,
            spend_timing.clone(),
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
        let spending_targets = SpendingTargetManager::from_records(
            governance
                .load_spending_targets()
                .context("load spending targets")?,
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
            auth,
            users: Mutex::new(users),
            registration: Mutex::new(
                RegistrationManager::open(&path).context("initialize agent registration store")?,
            ),
            spend,
            budgets: Mutex::new(budgets),
            spending_targets: Mutex::new(spending_targets),
            policies: Mutex::new(policies),
            governance: Mutex::new(governance),
            payment_attempts: Mutex::new(payment_attempts),
            payments: Mutex::new(payments),
            spend_timing,
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

fn load_spend_timing_config() -> Result<SpendTimingConfig> {
    let config = match env::var(SPEND_TIMING_CONFIG_ENV) {
        Ok(path) => {
            let contents = fs::read_to_string(&path)
                .with_context(|| format!("read spend timing config `{path}`"))?;
            parse_spend_timing_config(&contents, &path)?
        }
        Err(env::VarError::NotPresent) => SpendTimingConfig::default(),
        Err(error) => return Err(error).context("read spend timing config environment variable"),
    };
    config
        .validate()
        .map_err(|error| anyhow!("invalid spend timing config: {error}"))?;
    Ok(config)
}

fn parse_spend_timing_config(yaml: &str, path: &str) -> Result<SpendTimingConfig> {
    serde_yaml_ng::from_str(yaml).with_context(|| format!("parse spend timing config `{path}`"))
}

/// Local API authority for the current Hubu process.
///
/// This is intentionally a *baseline localhost hardening layer*, not a complete
/// money-grade authorization system. It is still meaningfully better than an
/// unauthenticated localhost API because protected routes reject callers that do
/// not possess the Hubu token, which blocks accidental external exposure, other
/// OS users when file permissions hold, and browser-origin blind writes that
/// cannot attach the bearer token. It also gives the server a concrete
/// authenticated owner instead of trusting a process-wide default user.
///
/// This does **not** defend against a malicious process running as the same OS
/// user that can read `HUBU_AUTH_TOKEN`, read the token file, or control an
/// already-authorized client. Real money movement should add scoped,
/// short-lived capabilities and a durable human approval authority so possession
/// of this local transport token alone is not enough to create budgets, change
/// policies, or settle money.
struct LocalAuth {
    token_hash: String,
    token_source: String,
    reconciliation_token_hash: String,
    reconciliation_token_source: String,
    owner_user_id: Mutex<UserId>,
}

impl LocalAuth {
    fn new(owner_user_id: UserId) -> Result<Self> {
        let token = load_local_auth_token()?;
        let reconciliation_token = load_local_reconciliation_token()?;
        if constant_time_eq(
            hash_token(&token.value).as_bytes(),
            hash_token(&reconciliation_token.value).as_bytes(),
        ) {
            return Err(anyhow!(
                "Hubu reconciliation capability must be distinct from the API bearer token"
            ));
        }
        Ok(Self {
            token_hash: hash_token(&token.value),
            token_source: token.source,
            reconciliation_token_hash: hash_token(&reconciliation_token.value),
            reconciliation_token_source: reconciliation_token.source,
            owner_user_id: Mutex::new(owner_user_id),
        })
    }

    fn verifies(&self, token: &str) -> bool {
        constant_time_eq(hash_token(token).as_bytes(), self.token_hash.as_bytes())
    }

    fn source(&self) -> &str {
        &self.token_source
    }

    fn verifies_reconciliation_capability(&self, token: &str) -> bool {
        constant_time_eq(
            hash_token(token).as_bytes(),
            self.reconciliation_token_hash.as_bytes(),
        )
    }

    fn reconciliation_source(&self) -> &str {
        &self.reconciliation_token_source
    }

    fn owner_user_id(&self) -> Result<UserId> {
        self.owner_user_id
            .lock()
            .map_err(|_| anyhow!("local API auth owner lock poisoned"))
            .map(|owner| owner.clone())
    }

    fn select_owner_user(&self, user_id: &UserId) -> Result<()> {
        *self
            .owner_user_id
            .lock()
            .map_err(|_| anyhow!("local API auth owner lock poisoned"))? = user_id.clone();
        Ok(())
    }
}

struct LoadedLocalAuthToken {
    value: String,
    source: String,
}

fn load_local_auth_token() -> Result<LoadedLocalAuthToken> {
    #[cfg(test)]
    if env::var(AUTH_TOKEN_ENV).is_err() && env::var(AUTH_TOKEN_FILE_ENV).is_err() {
        return Ok(LoadedLocalAuthToken {
            value: TEST_AUTH_TOKEN.to_string(),
            source: "test".to_string(),
        });
    }

    if let Ok(token) = env::var(AUTH_TOKEN_ENV) {
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err(anyhow!("{AUTH_TOKEN_ENV} cannot be empty"));
        }
        return Ok(LoadedLocalAuthToken {
            value: token,
            source: AUTH_TOKEN_ENV.to_string(),
        });
    }

    let path = auth_token_file_path();
    match fs::read_to_string(&path) {
        Ok(contents) => {
            let token = contents.trim().to_string();
            if token.is_empty() {
                return Err(anyhow!(
                    "Hubu auth token file `{}` is empty",
                    path.display()
                ));
            }
            Ok(LoadedLocalAuthToken {
                value: token,
                source: path.display().to_string(),
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let token = create_auth_token_file(&path)?;
            Ok(LoadedLocalAuthToken {
                value: token,
                source: path.display().to_string(),
            })
        }
        Err(error) => {
            Err(error).with_context(|| format!("read Hubu auth token file `{}`", path.display()))
        }
    }
}

fn load_local_reconciliation_token() -> Result<LoadedLocalAuthToken> {
    #[cfg(test)]
    if env::var(RECONCILIATION_TOKEN_ENV).is_err()
        && env::var(RECONCILIATION_TOKEN_FILE_ENV).is_err()
    {
        return Ok(LoadedLocalAuthToken {
            value: TEST_RECONCILIATION_TOKEN.to_string(),
            source: "test".to_string(),
        });
    }

    if let Ok(token) = env::var(RECONCILIATION_TOKEN_ENV) {
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err(anyhow!("{RECONCILIATION_TOKEN_ENV} cannot be empty"));
        }
        return Ok(LoadedLocalAuthToken {
            value: token,
            source: RECONCILIATION_TOKEN_ENV.to_string(),
        });
    }

    let path = reconciliation_token_file_path();
    match fs::read_to_string(&path) {
        Ok(contents) => {
            let token = contents.trim().to_string();
            if token.is_empty() {
                return Err(anyhow!(
                    "Hubu reconciliation token file `{}` is empty",
                    path.display()
                ));
            }
            Ok(LoadedLocalAuthToken {
                value: token,
                source: path.display().to_string(),
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let token = create_token_file(&path, "hubu_reconcile_")?;
            Ok(LoadedLocalAuthToken {
                value: token,
                source: path.display().to_string(),
            })
        }
        Err(error) => Err(error)
            .with_context(|| format!("read Hubu reconciliation token file `{}`", path.display())),
    }
}

fn auth_token_file_path() -> PathBuf {
    env::var(AUTH_TOKEN_FILE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_AUTH_TOKEN_FILE))
}

fn reconciliation_token_file_path() -> PathBuf {
    env::var(RECONCILIATION_TOKEN_FILE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_RECONCILIATION_TOKEN_FILE))
}

fn create_auth_token_file(path: &Path) -> Result<String> {
    create_token_file(path, "hubu_")
}

fn create_token_file(path: &Path, prefix: &str) -> Result<String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create auth token directory `{}`", parent.display()))?;
    }

    let token = format!("{prefix}{}", AgentSessionId::new());
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);

    match options.open(path) {
        Ok(mut file) => {
            writeln!(file, "{token}")
                .with_context(|| format!("write Hubu auth token file `{}`", path.display()))?;
            Ok(token)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let token = fs::read_to_string(path)
                .with_context(|| {
                    format!("read existing Hubu auth token file `{}`", path.display())
                })?
                .trim()
                .to_string();
            if token.is_empty() {
                Err(anyhow!(
                    "Hubu auth token file `{}` is empty",
                    path.display()
                ))
            } else {
                Ok(token)
            }
        }
        Err(error) => {
            Err(error).with_context(|| format!("create Hubu auth token file `{}`", path.display()))
        }
    }
}

fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    format!("{FINGERPRINT_PREFIX}{}", hex_encode(&digest))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }

    left.iter()
        .zip(right)
        .fold(0_u8, |diff, (left, right)| diff | (left ^ right))
        == 0
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
    username: Option<String>,
    display_name: String,
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InitHttpRequest {
    username: Option<String>,
    display_name: Option<String>,
    email: Option<String>,
}

#[derive(Debug, Serialize)]
struct InitHttpResponse {
    user_id: String,
    username: Option<String>,
    display_name: String,
}

#[derive(Debug, Serialize)]
struct UserListHttpResponse {
    users: Vec<UserHttpResponse>,
}

#[derive(Debug, Serialize)]
struct UserHttpResponse {
    user_id: String,
    username: Option<String>,
    display_name: String,
    email: Option<String>,
    status: String,
    current: bool,
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct AddPolicyHttpRequest {
    agent_id: Option<String>,
    daily_limit_cents: Option<i64>,
    policy_yaml: Option<String>,
}

#[derive(Debug, Serialize)]
struct AddPolicyHttpResponse {
    scope: String,
    agent_id: Option<String>,
    policy_id: String,
    policy_version: String,
    default_decision: String,
}

#[derive(Debug, Serialize)]
struct PolicyListHttpResponse {
    policies: Vec<PolicyHttpResponse>,
}

#[derive(Debug, Serialize)]
struct PolicyHttpResponse {
    scope: String,
    agent_id: Option<String>,
    policy_id: String,
    policy_version: String,
    default_decision: String,
    rules: usize,
    attached_at: String,
    updated_at: String,
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
    owner_user_id: String,
    owner_username: Option<String>,
    agent_type: String,
    status: String,
    account_id: String,
    account_status: String,
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct CreateBudgetHttpRequest {
    amount_cents: i64,
    agent_id: Option<String>,
    starting_at: Option<String>,
    ending_before: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateBudgetSeriesHttpRequest {
    amount_cents: i64,
    agent_id: Option<String>,
    starting_at: Option<String>,
    recurrence: BudgetRecurrenceHttp,
    period_count: usize,
}

#[derive(Debug, Deserialize)]
struct BudgetIdHttpRequest {
    budget_id: String,
}

#[derive(Debug, Deserialize)]
struct ReplaceBudgetHttpRequest {
    budget_id: String,
    amount_cents: i64,
}

#[derive(Debug, Deserialize)]
struct SetSpendingTargetHttpRequest {
    amount_cents: i64,
    starting_at: Option<String>,
    ending_before: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SpendingTargetIdHttpRequest {
    target_id: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BudgetRecurrenceHttp {
    Daily,
    Monthly,
    Yearly,
}

#[derive(Debug, Serialize)]
struct CreateBudgetHttpResponse {
    budget: BudgetHttpResponse,
    spending_target_warnings: Vec<SpendingTargetWarningHttpResponse>,
}

#[derive(Debug, Serialize)]
struct CreateBudgetSeriesHttpResponse {
    budgets: Vec<BudgetHttpResponse>,
    spending_target_warnings: Vec<SpendingTargetWarningHttpResponse>,
}

#[derive(Debug, Serialize)]
struct RevokeBudgetHttpResponse {
    budget: BudgetHttpResponse,
}

#[derive(Debug, Serialize)]
struct ReplaceBudgetHttpResponse {
    revoked_budget: BudgetHttpResponse,
    budget: BudgetHttpResponse,
    spending_target_warnings: Vec<SpendingTargetWarningHttpResponse>,
}

#[derive(Debug, Serialize)]
struct ListBudgetsHttpResponse {
    budgets: Vec<BudgetHttpResponse>,
}

#[derive(Debug, Serialize)]
struct SetSpendingTargetHttpResponse {
    target: SpendingTargetHttpResponse,
}

#[derive(Debug, Serialize)]
struct ListSpendingTargetsHttpResponse {
    targets: Vec<SpendingTargetHttpResponse>,
}

#[derive(Debug, Serialize)]
struct RevokeSpendingTargetHttpResponse {
    target: SpendingTargetHttpResponse,
}

#[derive(Debug, Serialize)]
struct BudgetHttpResponse {
    budget_id: String,
    agent_id: String,
    amount_limit_cents: i64,
    currency: String,
    starting_at: String,
    ending_before: Option<String>,
    status: String,
    consumed_amount_cents: i64,
    frozen_amount_cents: i64,
    remaining_amount_cents: i64,
}

#[derive(Debug, Serialize)]
struct SpendingTargetHttpResponse {
    target_id: String,
    target_amount_cents: i64,
    allocated_amount_cents: i64,
    exceeded_by_cents: i64,
    is_exceeded: bool,
    currency: String,
    starting_at: String,
    ending_before: Option<String>,
    status: String,
}

#[derive(Debug, Serialize)]
struct SpendingTargetWarningHttpResponse {
    target_id: String,
    target_amount_cents: i64,
    allocated_amount_cents: i64,
    exceeded_by_cents: i64,
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpendHttpRequest {
    operation_key: Option<String>,
    agent_id: Option<String>,
    account_id: Option<String>,
    amount_cents: i64,
    reason: String,
    merchant: Option<String>,
    workload_profile: Option<String>,
}

#[derive(Debug, Serialize)]
struct SpendHttpResponse {
    operation_key: String,
    account_id: String,
    agent_id: String,
    decision_id: String,
    decision: String,
    reasons: Vec<String>,
    auth_token_id: Option<String>,
    workload_profile: String,
    authorization_expires_at: Option<String>,
    budget_hold: Option<BudgetHoldHttpResponse>,
    payment: Option<PaymentHttpResponse>,
}

#[derive(Debug, Deserialize)]
struct ExecutorSpendHttpRequest {
    spend_auth_token_id: String,
    agent_id: Option<String>,
    account_id: Option<String>,
    amount_cents: i64,
    merchant: Option<String>,
    task_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExecutorSpendClaimHttpRequest {
    #[serde(flatten)]
    spend: ExecutorSpendHttpRequest,
    #[serde(alias = "executor_execution_id")]
    operation_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ExecutorSpendFinalizationHttpRequest {
    Executor(ExecutorSpendFinalizeHttpRequest),
    Reconciliation(ExecutorClaimReconciliationHttpRequest),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutorSpendFinalizeHttpRequest {
    #[serde(alias = "executor_execution_id")]
    operation_key: String,
    agent_id: String,
    receipt: Option<SpendExecutorSettlementReceipt>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutorClaimReconciliationHttpRequest {
    claim_id: String,
    provider_reference: String,
    evidence: String,
    receipt: Option<SpendExecutorSettlementReceipt>,
}

#[derive(Debug, Serialize)]
struct ExecutorSpendHttpResponse {
    operation_key: String,
    spend_auth_token_id: String,
    decision_id: String,
    account_id: String,
    agent_id: String,
    amount_cents: i64,
    currency: String,
    merchant: Option<String>,
    task_id: Option<String>,
    expires_at: String,
    budget_hold: BudgetHoldHttpResponse,
}

#[derive(Debug, Serialize)]
struct ExecutorSpendClaimHttpResponse {
    operation_key: String,
    claim_id: String,
    workload_profile: String,
    status: String,
    claimed_at: String,
    claim_expires_at: String,
    finalized_at: Option<String>,
    settlement_id: Option<String>,
    reconciliation_required: bool,
    reconciliation_outcome: Option<String>,
    provider_reference: Option<String>,
    evidence: Option<String>,
    reconciled_at: Option<String>,
    reconciled_by_user_id: Option<String>,
    spend: ExecutorSpendHttpResponse,
}

#[derive(Debug, Serialize)]
struct ExecutorClaimsHttpResponse {
    claims: Vec<ExecutorSpendClaimHttpResponse>,
}

#[derive(Debug, Serialize)]
struct ExecutorSpendSettlementHttpResponse {
    operation_key: String,
    settlement_id: String,
    claim_id: String,
    status: String,
    receipt: ExecutorSpendSettlementReceiptHttpResponse,
    spend: ExecutorSpendHttpResponse,
}

#[derive(Debug, Serialize)]
struct ExecutorSpendSettlementReceiptHttpResponse {
    authorized_max_cents: i64,
    actual_vendor_cost_cents: i64,
    released_amount_cents: i64,
    currency: String,
    provider_request_id: String,
    price_model_snapshot: SpendExecutorPriceModelSnapshot,
    artifact_reference: String,
    created_at: String,
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
    let raw = read_http_request(&mut stream, started_at + HTTP_READ_TIMEOUT)?;
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

fn read_http_request(stream: &mut TcpStream, deadline: Instant) -> Result<String> {
    read_http_request_with_guard(stream, |stream| {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| anyhow!("HTTP request read deadline exceeded"))?;
        stream
            .set_read_timeout(Some(remaining))
            .context("set remaining HTTP request read timeout")
    })
}

fn read_http_request_with_guard<R: Read>(
    reader: &mut R,
    mut prepare_read: impl FnMut(&mut R) -> Result<()>,
) -> Result<String> {
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 8192];
    let header_end = loop {
        if let Some(boundary) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            let header_end = boundary + 4;
            if header_end > MAX_HTTP_HEADER_BYTES {
                return Err(anyhow!("HTTP request headers exceed size limit"));
            }
            break header_end;
        }
        if raw.len() >= MAX_HTTP_HEADER_BYTES {
            return Err(anyhow!("HTTP request headers exceed size limit"));
        }

        prepare_read(reader)?;
        let bytes_read = reader.read(&mut chunk)?;
        if bytes_read == 0 {
            if raw.is_empty() {
                return Ok(String::new());
            }
            return Err(anyhow!("incomplete HTTP request headers"));
        }
        raw.extend_from_slice(&chunk[..bytes_read]);
    };

    let head = std::str::from_utf8(&raw[..header_end - 4])
        .context("HTTP request headers are not valid UTF-8")?;
    let content_length = declared_content_length(head)?;
    if content_length > MAX_HTTP_BODY_BYTES {
        return Err(anyhow!("HTTP request body exceeds size limit"));
    }

    let request_len = header_end + content_length;
    while raw.len() < request_len {
        prepare_read(reader)?;
        let remaining = request_len - raw.len();
        let read_capacity = remaining.min(chunk.len());
        let bytes_read = reader
            .read(&mut chunk[..read_capacity])
            .context("read HTTP request body")?;
        if bytes_read == 0 {
            return Err(anyhow!("incomplete HTTP request body"));
        }
        raw.extend_from_slice(&chunk[..bytes_read]);
    }
    raw.truncate(request_len);

    String::from_utf8(raw).context("HTTP request is not valid UTF-8")
}

fn declared_content_length(head: &str) -> Result<usize> {
    let mut content_length = None;
    for line in head.split("\r\n").skip(1) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| anyhow!("malformed HTTP header"))?;
        if name.trim().eq_ignore_ascii_case("transfer-encoding") {
            return Err(anyhow!("Transfer-Encoding is not supported"));
        }
        if name.trim().eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(anyhow!("duplicate Content-Length header"));
            }
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .context("invalid Content-Length header")?,
            );
        }
    }
    Ok(content_length.unwrap_or(0))
}

fn route(request: HttpRequest, state: &ServerState) -> HttpResponse {
    if !is_public_route(&request) {
        if let Err(error) = authenticate_request(&request, state) {
            log_event(
                "warn",
                "http_request_unauthorized",
                json!({
                    "method": request.method,
                    "path": request.path,
                    "error": error.to_string(),
                }),
            );
            return HttpResponse {
                status: 401,
                body: json!({ "error": error.to_string() }),
            };
        }
    }

    let reconciliation_capability = request
        .headers
        .get(RECONCILIATION_CAPABILITY_HEADER)
        .map(String::as_str);
    let result = match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") => Ok(json!({ "status": "ok" })),
        ("GET", "/version") => serde_json::to_value(build_info()).map_err(Into::into),
        ("GET", "/registration/guidance")
        | ("GET", "/.well-known/hubu-agent-registration.json") => Ok(registration_guidance()),
        ("GET", "/user") => current_user(state).map(to_json),
        ("GET", "/users") => list_users(state).map(to_json),
        ("POST", "/init") => init(request.body, state).map(to_json),
        ("POST", "/user/spending-target") => set_spending_target(request.body, state).map(to_json),
        ("GET", "/user/spending-target") => {
            list_spending_targets(state, query_flag(&request, "all")).map(to_json)
        }
        ("POST", "/user/spending-target/revoke") => {
            revoke_spending_target(request.body, state).map(to_json)
        }
        ("POST", "/agents/register") => register_agent(request.body, state).map(to_json),
        ("GET", "/agents") if query_flag(&request, "all") => {
            list_agents_for_scope(state, true).map(to_json)
        }
        ("GET", "/agents") => list_agents(state).map(to_json),
        ("GET", "/policies") => list_policies(state).map(to_json),
        ("POST", "/policies") => add_policy(request.body, state).map(to_json),
        ("POST", "/budgets") => create_budget(request.body, state).map(to_json),
        ("POST", "/budgets/series") => create_budget_series(request.body, state).map(to_json),
        ("POST", "/budgets/revoke") => revoke_budget(request.body, state).map(to_json),
        ("POST", "/budgets/replace") => replace_budget(request.body, state).map(to_json),
        ("GET", "/budgets") => list_budgets(state, query_flag(&request, "all")).map(to_json),
        ("POST", "/spend/authorize") => authorize_spend(request.body, state).map(to_json),
        ("GET", "/spend/executor/guidance") | ("GET", "/.well-known/hubu-spend-executor.json") => {
            Ok(spend_executor_guidance(state))
        }
        ("POST", "/spend/executor/validate") => {
            validate_executor_spend(request.body, state).map(to_json)
        }
        ("POST", "/spend/executor/claim") => claim_executor_spend(request.body, state).map(to_json),
        ("GET", "/spend/executor/claim") => {
            get_executor_claim(request.query.get("claim_id").map(String::as_str), state)
                .map(to_json)
        }
        ("GET", "/spend/executor/reconciliation") => {
            list_executor_claims_requiring_reconciliation(state).map(to_json)
        }
        ("POST", "/spend/executor/settle") => {
            finalize_executor_spend(request.body, state, true, reconciliation_capability)
        }
        ("POST", "/spend/executor/release") => {
            finalize_executor_spend(request.body, state, false, reconciliation_capability)
        }
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

fn is_public_route(request: &HttpRequest) -> bool {
    matches!(
        (request.method.as_str(), request.path.as_str()),
        ("GET", "/health")
            | ("GET", "/version")
            | ("GET", "/registration/guidance")
            | ("GET", "/.well-known/hubu-agent-registration.json")
            | ("GET", "/spend/executor/guidance")
            | ("GET", "/.well-known/hubu-spend-executor.json")
    )
}

// Defense in depth around the bearer token: require JSON POSTs, reject browser
// origins, and keep protected traffic on loopback hosts. These checks reduce
// common localhost attack paths, but the token remains the actual local
// capability and should not be treated as human approval for real-money flows.
fn authenticate_request(request: &HttpRequest, state: &ServerState) -> Result<()> {
    if request.headers.contains_key("origin") {
        return Err(anyhow!(
            "browser-origin requests are not accepted by the Hubu local API"
        ));
    }

    if request.method == "POST" {
        let content_type = request
            .headers
            .get("content-type")
            .ok_or_else(|| anyhow!("POST requests require Content-Type: application/json"))?;
        if !content_type
            .split(';')
            .next()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
        {
            return Err(anyhow!(
                "POST requests require Content-Type: application/json"
            ));
        }
    }

    if let Some(host) = request.headers.get("host") {
        let host = host.trim();
        let is_loopback = host == "localhost"
            || host.starts_with("localhost:")
            || host == "127.0.0.1"
            || host.starts_with("127.0.0.1:")
            || host == "::1"
            || host == "[::1]"
            || host.starts_with("[::1]:");
        if !is_loopback {
            return Err(anyhow!("Hubu local API only accepts loopback Host headers"));
        }
    }

    let authorization = request
        .headers
        .get("authorization")
        .ok_or_else(|| anyhow!("missing authorization bearer token"))?;
    let (scheme, token) = authorization
        .split_once(' ')
        .ok_or_else(|| anyhow!("invalid authorization bearer token"))?;
    if !scheme.eq_ignore_ascii_case("bearer")
        || token.trim().is_empty()
        || !state.auth.verifies(token.trim())
    {
        return Err(anyhow!("invalid authorization bearer token"));
    }

    Ok(())
}

fn authenticate_reconciliation_capability(
    capability: Option<&str>,
    state: &ServerState,
) -> Result<()> {
    let capability = capability
        .map(str::trim)
        .filter(|capability| !capability.is_empty())
        .ok_or_else(|| anyhow!("missing human reconciliation capability"))?;
    if !state.auth.verifies_reconciliation_capability(capability) {
        return Err(anyhow!("invalid human reconciliation capability"));
    }
    Ok(())
}

fn spend_executor_guidance(state: &ServerState) -> Value {
    json!({
        "protocol_version": EXECUTOR_CONTRACT,
        "role_boundary": {
            "hubu": [
                "register agents and owners",
                "evaluate policy",
                "reserve one agent-budget hold per spend decision",
                "exclusively claim executor spend authorization",
                "settle actual vendor cost and release unused authorization, or release the full reserved budget"
            ],
            "executor": [
                "hold vendor credentials outside Hubu",
                "perform work such as model calls or API operations",
                "claim authorization before vendor work and finalize the claim afterward"
            ]
        },
        "executor_flow": [
            "platform supplies one stable operation_key and requests POST /spend/authorize with merchant, task scope, and workload_profile",
            "Hubu durably records the workflow under the agent-scoped operation_key",
            "agent sends the operation_key, spend_auth_token_id, and matching scope to an executor",
            "executor calls POST /spend/executor/claim with the same operation_key before irreversible work",
            "executor performs work with its own credentials",
            "executor finalizes by agent_id and operation_key with a provider receipt after successful irreversible work or releases before work is performed"
        ],
        "routes": {
            "guidance": [
                "GET /spend/executor/guidance",
                "GET /.well-known/hubu-spend-executor.json"
            ],
            "claim": "POST /spend/executor/claim",
            "claim_status": "GET /spend/executor/claim?claim_id=CLAIM_ID",
            "validate": "POST /spend/executor/validate",
            "settle": "POST /spend/executor/settle",
            "release": "POST /spend/executor/release",
            "reconciliation_queue": "GET /spend/executor/reconciliation",
            "reconcile_vendor_billed": "POST /spend/executor/settle",
            "reconcile_vendor_did_not_bill": "POST /spend/executor/release"
        },
        "operation_key_policy": {
            "responsibility": "agent platform or orchestrator, not the autonomous model",
            "namespace": [
                "agent_id",
                "operation_key"
            ],
            "generation": {
                "preferred": "reuse the platform's durable tool-call, run-step, or operation id with a platform prefix",
                "fallback": "the platform adapter generates and persists an opaque operation key before the first authorization attempt",
                "comparison": "case-sensitive after trimming surrounding whitespace"
            },
            "persistence": [
                "Hubu is the authoritative store for workflow state under the agent-scoped operation_key",
                "the client must reuse its stable operation_key for authorization, claim, finalization, and retries",
                "do not rely on model conversation memory for the operation_key"
            ],
            "prohibited": [
                "ask the autonomous model to invent the operation_key",
                "derive operation_key from mutable spend fields such as amount or merchant",
                "reuse one operation_key for different work by the same agent",
                "generate a new operation_key for a retry"
            ]
        },
        "authorization_request": {
            "required": [
                "operation_key",
                "account_id",
                "amount_cents",
                "reason"
            ],
            "optional": [
                "merchant",
                "workload_profile"
            ]
        },
        "claim_request": {
            "required": [
                "spend_auth_token_id",
                "operation_key",
                "account_id",
                "amount_cents"
            ],
            "optional": [
                "merchant",
                "task_id"
            ],
            "currency": "usd in v4"
        },
        "settle_request": {
            "required": [
                "agent_id",
                "operation_key",
                "receipt.actual_vendor_cost_cents",
                "receipt.provider_request_id",
                "receipt.price_model_snapshot",
                "receipt.artifact_reference"
            ]
        },
        "release_request": {
            "required": [
                "agent_id",
                "operation_key"
            ]
        },
        "reconciliation_request": {
            "required": [
                "claim_id",
                "provider_reference",
                "evidence"
            ],
            "vendor_billed_required": [
                "receipt.actual_vendor_cost_cents",
                "receipt.provider_request_id",
                "receipt.price_model_snapshot",
                "receipt.artifact_reference"
            ],
            "routes": {
                "vendor_billed": "POST /spend/executor/settle",
                "vendor_did_not_bill": "POST /spend/executor/release"
            },
            "human_gate": {
                "capability_header": RECONCILIATION_CAPABILITY_HEADER,
                "requirement": "The server requires a reconciliation capability distinct from the normal API bearer token.",
                "cli": "The CLI reads HUBU_RECONCILIATION_TOKEN or HUBU_RECONCILIATION_TOKEN_FILE and sends it only for reconciliation.",
                "mcp": "MCP reconciliation tools require both a trusted client approval prompt and the distinct reconciliation capability."
            }
        },
        "timing": &state.spend_timing,
        "scope_rules": [
            "operation_key is platform-assigned, immutable, and scoped to the authorized agent; authorization retries must use the same spend scope",
            "account_id, amount_cents, merchant, and task_id must match the original authorized spend",
            "workload_profile is selected during authorization and cannot be changed by the executor",
            "the spend auth token must be unexpired, unused, unrevoked, and unclaimed when a new claim starts",
            "authorization and claim retries with the same operation_key return stored workflow state, including terminal state",
            "claiming moves the associated budget hold from frozen to claimed and extends it to claim_expires_at",
            "an active claim remains finalizable after the original authorization expires",
            "Hubu does not accept vendor API keys or model/provider payloads in this protocol"
        ],
        "settlement_rules": [
            "settle only after the executor has performed irreversible billable work",
            "release only before irreversible billable work has occurred",
            "settlement atomically persists the immutable provider receipt, marks the claim settled and token used, consumes actual vendor cost, and releases the authorization remainder",
            "the actual vendor cost must be non-negative and cannot exceed the authorized maximum",
            "an identical settlement retry returns the original settlement_id and receipt without consuming budget twice; a changed receipt is rejected",
            "finalization resolves by agent_id and operation_key, so a caller can recover the result even if it lost the claim response",
            "claim expiry is evaluated once when the settlement transaction starts",
            "release atomically marks the claim released and token revoked while returning the reserved amount",
            "settle and release serialize so the first terminal finalization wins",
            "expired claims keep their holds claimed for reconciliation instead of automatically releasing",
            "claim status lookup reports reconciliation_required once a claimed lease expires",
            "only expired claimed leases enter the reconciliation queue",
            "the normal API bearer token cannot authorize reconciliation",
            "a human-confirmed vendor_billed resolution settles the hold; vendor_did_not_bill releases it",
            "each reconciliation stores the provider reference, evidence, resolving user, outcome, and timestamp"
        ],
        "merchant_examples": [
            "gongbu.image",
            "gongbu.browser",
            "example.executor"
        ]
    })
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
            username: None,
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
            username: request.username,
            display_name: request
                .display_name
                .unwrap_or_else(|| "Hubu User".to_string()),
            email: request.email,
        })?;
    state.auth.select_owner_user(&user.id)?;

    Ok(InitHttpResponse {
        user_id: user.pub_id,
        username: user.username,
        display_name: user.display_name,
    })
}

fn current_user(state: &ServerState) -> Result<CurrentUserHttpResponse> {
    let user = authenticated_user(state)?;
    Ok(CurrentUserHttpResponse {
        user_id: user.pub_id,
        username: user.username,
        display_name: user.display_name,
        email: user.email,
    })
}

fn list_users(state: &ServerState) -> Result<UserListHttpResponse> {
    let current_user_id = state.auth.owner_user_id()?;
    let users = state
        .users
        .lock()
        .map_err(|_| anyhow!("user manager lock poisoned"))?
        .list_users()?
        .into_iter()
        .map(|user| user_http_response(user, &current_user_id))
        .collect();
    Ok(UserListHttpResponse { users })
}

fn user_http_response(user: User, current_user_id: &UserId) -> UserHttpResponse {
    let current = user.id == *current_user_id;
    UserHttpResponse {
        user_id: user.pub_id,
        username: user.username,
        display_name: user.display_name,
        email: user.email,
        status: match user.status {
            hubu_common::models::UserStatus::Active => "active".to_string(),
            hubu_common::models::UserStatus::Suspended => "suspended".to_string(),
        },
        current,
        created_at: user.created_at.to_rfc3339(),
    }
}

fn register_agent(body: String, state: &ServerState) -> Result<RegisterAgentHttpResponse> {
    let request: RegisterAgentHttpRequest = serde_json::from_str(&body)?;
    let (request_shape, envelope) = match request {
        RegisterAgentHttpRequest::Envelope(envelope) => ("envelope", envelope),
        RegisterAgentHttpRequest::Simple(request) => (
            "simple",
            simple_registration_envelope(
                &request.name,
                &request.version,
                &authenticated_user(state)?.pub_id,
            ),
        ),
    };
    log_event(
        "info",
        "agent_registration_received",
        json!({
            "request_shape": request_shape,
            "protocol_version": envelope.protocol_version,
        }),
    );
    let user = authenticated_user(state)?;
    let registration_request = registration_request_from_envelope(envelope, &user)?;
    ensure_agent_not_already_registered(&registration_request, state)?;

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

fn ensure_agent_not_already_registered(
    request: &RegisterAgentRequest,
    state: &ServerState,
) -> Result<()> {
    let already_registered = state
        .registration
        .lock()
        .map_err(|_| anyhow!("registration manager lock poisoned"))?
        .agents_for_user(&request.owner_user_id)?
        .into_iter()
        .any(|agent| agent.agent.fingerprint == request.identity_fingerprint);
    if already_registered {
        return Err(anyhow!("agent is already registered for this owner"));
    }
    Ok(())
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
            description: Some("Registered through the Hubu CLI".to_string()),
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
    let owner_pub_id = owner
        .get("pub_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("identity payload owner.pub_id must be a string"))?;
    if owner_pub_id != user.pub_id {
        return Err(anyhow!(
            "identity payload owner.pub_id must match the current Hubu user"
        ));
    }
    log_event(
        "info",
        "agent_registration_fingerprints_verified",
        json!({
            "user_id": user.id.to_string(),
            "user_pub_id": user.pub_id,
            "identity_fingerprint": identity_fingerprint,
            "version_fingerprint": version_fingerprint,
        }),
    );

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

    let user = authenticated_user_context(state)?;
    let (scope, agent_pub_id) = if let Some(agent_pub_id) = request.agent_id {
        let agent_id = resolve_agent_id_for_user(&agent_pub_id, &user, state)?;
        (
            PolicyAssignmentScope::AgentOverride(agent_id),
            Some(agent_pub_id),
        )
    } else {
        (PolicyAssignmentScope::UserDefault, None)
    };
    let policy = if let Some(policy_yaml) = request.policy_yaml {
        let mut policy = Policy::from_yaml_str(&policy_yaml)
            .map_err(|error| anyhow!("{}", policy_load_error_message(error)))?;
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
            id: match agent_pub_id.as_deref() {
                Some(agent_pub_id) => format!("starter_policy_{agent_pub_id}"),
                None => "starter_policy_default".to_string(),
            },
            version: "starter-1".to_string(),
            owner_user_id: user.user_id.clone(),
            default_effect: Effect::NeedsApproval,
            rules: vec![
                Rule {
                    id: "deny_blocked_merchant".to_string(),
                    effect: Effect::Deny,
                    when: Condition::Eq {
                        field: Field::Merchant,
                        value: PolicyValue::String("blocked-merchant".to_string()),
                    },
                    reason: "merchant is blocked by the starter policy".to_string(),
                },
                Rule {
                    id: "allow_within_starter_limit".to_string(),
                    effect: Effect::Allow,
                    when: Condition::Lte {
                        field: Field::Amount,
                        value: PolicyValue::MoneyCents(daily_limit_cents),
                    },
                    reason: format!(
                        "amount is within the configured single-spend limit of {} cents",
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
        .save_policy_assignment(&user.user_id, &scope, &policy)?;

    state
        .policies
        .lock()
        .map_err(|_| anyhow!("policy store lock poisoned"))?
        .insert((user.user_id, scope.clone()), policy);

    log_event(
        "info",
        "policy_added",
        json!({
            "scope": scope.scope_type(),
            "agent_pub_id": agent_pub_id,
            "policy_id": policy_id,
            "policy_version": policy_version,
            "default_decision": default_decision,
        }),
    );
    Ok(AddPolicyHttpResponse {
        scope: scope.scope_type().to_string(),
        agent_id: agent_pub_id,
        policy_id,
        policy_version,
        default_decision,
    })
}

fn list_policies(state: &ServerState) -> Result<PolicyListHttpResponse> {
    let user = authenticated_user_context(state)?;
    let assignments = state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?
        .load_policy_assignments()?;

    let policies = assignments
        .into_iter()
        .filter(|assignment| assignment.owner_user_id == user.user_id)
        .map(|assignment| policy_http_response(assignment, state))
        .collect::<Result<Vec<_>>>()?;

    Ok(PolicyListHttpResponse { policies })
}

fn policy_http_response(
    assignment: hubu_core::persistence::PolicyAssignmentRecord,
    state: &ServerState,
) -> Result<PolicyHttpResponse> {
    let agent_id = match &assignment.scope {
        PolicyAssignmentScope::UserDefault => None,
        PolicyAssignmentScope::AgentOverride(agent_id) => {
            Some(registration_agent_pub_id(agent_id, state)?)
        }
    };
    Ok(PolicyHttpResponse {
        scope: assignment.scope.scope_type().to_string(),
        agent_id,
        policy_id: assignment.policy.id,
        policy_version: assignment.policy.version,
        default_decision: effect_name(assignment.policy.default_effect).to_string(),
        rules: assignment.policy.rules.len(),
        attached_at: assignment.created_at.to_rfc3339(),
        updated_at: assignment.updated_at.to_rfc3339(),
    })
}

fn policy_load_error_message(error: PolicyLoadError) -> String {
    match error {
        PolicyLoadError::ReadFile { path, source } => {
            format!("failed to read policy file `{path}`: {source}")
        }
        PolicyLoadError::ParseYaml { source } => {
            format!("failed to parse policy yaml: {source}")
        }
        PolicyLoadError::Validation { source } => {
            format!("invalid policy: {source}")
        }
    }
}

fn list_agents(state: &ServerState) -> Result<AgentListHttpResponse> {
    list_agents_for_scope(state, false)
}

fn list_agents_for_scope(state: &ServerState, include_all: bool) -> Result<AgentListHttpResponse> {
    let user = authenticated_user_context(state)?;
    if include_all {
        return list_all_agents(state);
    }
    let owner = owner_metadata_for_user_id(&user.user_id, state)?;
    let agents = state
        .registration
        .lock()
        .map_err(|_| anyhow!("registration manager lock poisoned"))?
        .agents_for_user(&user.user_id)?
        .into_iter()
        .map(|agent| agent_http_response(agent, &owner))
        .collect();

    Ok(AgentListHttpResponse { agents })
}

fn list_all_agents(state: &ServerState) -> Result<AgentListHttpResponse> {
    let users = state
        .users
        .lock()
        .map_err(|_| anyhow!("user manager lock poisoned"))?
        .list_users()?;
    let registration = state
        .registration
        .lock()
        .map_err(|_| anyhow!("registration manager lock poisoned"))?;
    let mut agents = Vec::new();
    for user in users {
        let owner = OwnerHttpMetadata {
            pub_id: user.pub_id,
            username: user.username,
            display_name: user.display_name,
        };
        agents.extend(
            registration
                .agents_for_user(&user.id)?
                .into_iter()
                .map(|agent| agent_http_response(agent, &owner)),
        );
    }
    Ok(AgentListHttpResponse { agents })
}

fn agent_http_response(agent: AgentWithAccount, owner: &OwnerHttpMetadata) -> AgentHttpResponse {
    AgentHttpResponse {
        agent_id: agent.agent.pub_id,
        display_name: agent.agent.display_name,
        description: agent.agent.description,
        owner_user_id: owner.pub_id.clone(),
        owner_username: owner.username.clone(),
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
    }
}

fn create_budget(body: String, state: &ServerState) -> Result<CreateBudgetHttpResponse> {
    let request: CreateBudgetHttpRequest = serde_json::from_str(&body)?;
    if request.amount_cents <= 0 {
        return Err(anyhow!("budget amount must be positive"));
    }

    let user = authenticated_user_context(state)?;
    let agent_id = required_budget_agent_id(request.agent_id.as_deref(), &user, state)?;
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
            agent_id: agent_id.clone(),
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
            "agent_id": agent_id.to_string(),
            "amount_cents": response.budget.amount_limit_cents,
            "currency": response.budget.currency.to_string(),
            "starting_at": response.budget.period.starting_at.to_rfc3339(),
            "ending_before": response.budget.period.ending_before.map(|value| value.to_rfc3339()),
        }),
    );
    let budget = BudgetWithBalance {
        budget: response.budget,
        balance: response.balance,
    };
    let spending_target_warnings = spending_target_warnings_for_periods(
        &user,
        std::slice::from_ref(&budget.budget.period),
        budget.budget.currency,
        state,
    )?;
    Ok(CreateBudgetHttpResponse {
        budget: budget_response(budget, state)?,
        spending_target_warnings,
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

    let user = authenticated_user_context(state)?;
    let agent_id = required_budget_agent_id(request.agent_id.as_deref(), &user, state)?;
    let starting_at = parse_optional_datetime(request.starting_at)?.unwrap_or_else(Utc::now);
    let recurrence = budget_recurrence(request.recurrence);
    budget_series_periods(starting_at, recurrence, request.period_count)?;

    let response = state
        .budgets
        .lock()
        .map_err(|_| anyhow!("budget manager lock poisoned"))?
        .create_budget_series(CreateBudgetSeriesRequest {
            agent_id: agent_id.clone(),
            amount_limit_cents: request.amount_cents,
            currency: Currency::Usd,
            starting_at,
            recurrence,
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
            "agent_id": agent_id.to_string(),
        }),
    );
    let periods = response
        .budgets
        .iter()
        .map(|budget| budget.budget.period.clone())
        .collect::<Vec<_>>();
    let spending_target_warnings =
        spending_target_warnings_for_periods(&user, &periods, Currency::Usd, state)?;
    Ok(CreateBudgetSeriesHttpResponse {
        budgets: response
            .budgets
            .into_iter()
            .map(|budget| budget_response(budget, state))
            .collect::<Result<Vec<_>>>()?,
        spending_target_warnings,
    })
}

fn set_spending_target(body: String, state: &ServerState) -> Result<SetSpendingTargetHttpResponse> {
    let request: SetSpendingTargetHttpRequest = serde_json::from_str(&body)?;
    if request.amount_cents <= 0 {
        return Err(anyhow!("spending target amount must be positive"));
    }

    let user = authenticated_user_context(state)?;
    let period = TimePeriod::new(
        parse_optional_datetime(request.starting_at)?.unwrap_or_else(Utc::now),
        parse_optional_datetime(request.ending_before)?,
    )
    .map_err(|error| anyhow!("invalid spending target period: {error:?}"))?;
    let response = state
        .spending_targets
        .lock()
        .map_err(|_| anyhow!("spending target manager lock poisoned"))?
        .create_target(CreateSpendingTargetRequest {
            owner_user_id: user.user_id.clone(),
            target_amount_cents: request.amount_cents,
            currency: Currency::Usd,
            period,
        })?;
    state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?
        .save_spending_target(&response)?;

    Ok(SetSpendingTargetHttpResponse {
        target: spending_target_response(response, state)?,
    })
}

fn list_spending_targets(
    state: &ServerState,
    include_all: bool,
) -> Result<ListSpendingTargetsHttpResponse> {
    let user = authenticated_user_context(state)?;
    let targets = {
        let manager = state
            .spending_targets
            .lock()
            .map_err(|_| anyhow!("spending target manager lock poisoned"))?;
        manager.get_targets_by_user_id(&user.user_id)
    };
    let targets = targets
        .into_iter()
        .filter(|target| {
            include_all
                || (target.status == SpendingTargetStatus::Active
                    && target
                        .period
                        .ending_before
                        .map_or(true, |ending_before| ending_before > Utc::now()))
        })
        .map(|target| spending_target_response(target, state))
        .collect::<Result<Vec<_>>>()?;

    Ok(ListSpendingTargetsHttpResponse { targets })
}

fn revoke_spending_target(
    body: String,
    state: &ServerState,
) -> Result<RevokeSpendingTargetHttpResponse> {
    let request: SpendingTargetIdHttpRequest = serde_json::from_str(&body)?;
    let user = authenticated_user_context(state)?;
    let target_id = resolve_spending_target_id(&request.target_id, &user, state)?;
    let revoked = {
        let mut targets = state
            .spending_targets
            .lock()
            .map_err(|_| anyhow!("spending target manager lock poisoned"))?;
        targets.revoke_target(&target_id)?
    };
    state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?
        .save_spending_target(&revoked)?;

    Ok(RevokeSpendingTargetHttpResponse {
        target: spending_target_response(revoked, state)?,
    })
}

fn list_budgets(state: &ServerState, include_all: bool) -> Result<ListBudgetsHttpResponse> {
    let user = authenticated_user_context(state)?;
    reconcile_expired_budget_holds(state)?;
    let budgets = budgets_for_user(&user, state)?;
    let budgets = budgets
        .into_iter()
        .filter(|budget| include_all || matches!(budget.budget.status, BudgetStatus::Active))
        .map(|budget| budget_response(budget, state))
        .collect::<Result<Vec<_>>>()?;

    Ok(ListBudgetsHttpResponse { budgets })
}

fn budgets_for_user(user: &UserContext, state: &ServerState) -> Result<Vec<BudgetWithBalance>> {
    let agent_ids = {
        let registration = state
            .registration
            .lock()
            .map_err(|_| anyhow!("registration manager lock poisoned"))?;
        registration
            .agents_for_user(&user.user_id)?
            .into_iter()
            .map(|agent| agent.agent.id)
            .collect::<Vec<_>>()
    };
    let manager = state
        .budgets
        .lock()
        .map_err(|_| anyhow!("budget manager lock poisoned"))?;
    let mut budgets = Vec::new();
    for agent_id in agent_ids {
        budgets.extend(manager.get_budgets_by_agent_id(&agent_id));
    }
    Ok(budgets)
}

fn resolve_budget_id_for_user(
    budget_pub_id: &str,
    user: &UserContext,
    state: &ServerState,
) -> Result<BudgetId> {
    budgets_for_user(user, state)?
        .into_iter()
        .find(|budget| public_budget_id(&budget.budget.id) == budget_pub_id)
        .map(|budget| budget.budget.id)
        .ok_or_else(|| anyhow!("unknown budget id {budget_pub_id}"))
}

fn resolve_spending_target_id(
    target_pub_id: &str,
    user: &UserContext,
    state: &ServerState,
) -> Result<SpendingTargetId> {
    let manager = state
        .spending_targets
        .lock()
        .map_err(|_| anyhow!("spending target manager lock poisoned"))?;
    manager
        .get_targets_by_user_id(&user.user_id)
        .into_iter()
        .find(|target| public_spending_target_id(&target.id) == target_pub_id)
        .map(|target| target.id)
        .ok_or_else(|| anyhow!("unknown spending target id {target_pub_id}"))
}

fn revoke_budget(body: String, state: &ServerState) -> Result<RevokeBudgetHttpResponse> {
    let request: BudgetIdHttpRequest = serde_json::from_str(&body)?;
    let user = authenticated_user_context(state)?;
    let budget_id = resolve_budget_id_for_user(&request.budget_id, &user, state)?;
    let revoked = {
        let mut budgets = state
            .budgets
            .lock()
            .map_err(|_| anyhow!("budget manager lock poisoned"))?;
        budgets.revoke_budget(&budget_id)?
    };
    state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?
        .save_budget_with_balance(&revoked.budget, &revoked.balance)?;

    Ok(RevokeBudgetHttpResponse {
        budget: budget_response(revoked, state)?,
    })
}

fn replace_budget(body: String, state: &ServerState) -> Result<ReplaceBudgetHttpResponse> {
    let request: ReplaceBudgetHttpRequest = serde_json::from_str(&body)?;
    if request.amount_cents <= 0 {
        return Err(anyhow!("budget amount must be positive"));
    }

    let user = authenticated_user_context(state)?;
    let budget_id = resolve_budget_id_for_user(&request.budget_id, &user, state)?;
    let original = state
        .budgets
        .lock()
        .map_err(|_| anyhow!("budget manager lock poisoned"))?
        .get_budget_by_id(&budget_id)
        .ok_or_else(|| anyhow!("unknown budget id {}", request.budget_id))?;

    if !matches!(original.budget.status, BudgetStatus::Active) {
        return Err(anyhow!("budget {} is not active", request.budget_id));
    }
    if original.balance.frozen_amount_cents > 0 {
        return Err(anyhow!("budget {} has frozen holds", request.budget_id));
    }

    let now = Utc::now();
    let ending_before = original.budget.period.ending_before;
    if ending_before.is_some_and(|ending_before| ending_before <= now) {
        return Err(anyhow!(
            "budget {} has no remaining period",
            request.budget_id
        ));
    }
    let replacement_starting_at = original.budget.period.starting_at.max(now);
    let replacement_period = TimePeriod::new(replacement_starting_at, ending_before)
        .map_err(|error| anyhow!("invalid replacement budget period: {error:?}"))?;

    let (revoked, created) = {
        let mut budgets = state
            .budgets
            .lock()
            .map_err(|_| anyhow!("budget manager lock poisoned"))?;
        let revoked = budgets.revoke_budget(&budget_id)?;
        let created = budgets.create_single_budget(CreateSingleBudgetRequest {
            agent_id: original.budget.agent_id,
            amount_limit_cents: request.amount_cents,
            currency: original.budget.currency,
            period: replacement_period,
        })?;
        (
            revoked,
            BudgetWithBalance {
                budget: created.budget,
                balance: created.balance,
            },
        )
    };
    {
        let mut governance = state
            .governance
            .lock()
            .map_err(|_| anyhow!("governance store lock poisoned"))?;
        governance.save_budget_with_balance(&revoked.budget, &revoked.balance)?;
        governance.save_budget_with_balance(&created.budget, &created.balance)?;
    }

    let spending_target_warnings = spending_target_warnings_for_periods(
        &user,
        std::slice::from_ref(&created.budget.period),
        created.budget.currency,
        state,
    )?;
    Ok(ReplaceBudgetHttpResponse {
        revoked_budget: budget_response(revoked, state)?,
        budget: budget_response(created, state)?,
        spending_target_warnings,
    })
}

fn authorize_spend(body: String, state: &ServerState) -> Result<SpendHttpResponse> {
    let request: SpendHttpRequest = serde_json::from_str(&body)?;
    let authorization = match evaluate_and_reserve_spend(request, state)? {
        SpendAuthorization::Authorized(authorization) => authorization,
        SpendAuthorization::Response(response) => return Ok(response),
    };
    let auth_token_id = authorization.approval.auth_token_id();
    let workload_profile = authorization.approval.workload_profile.clone();
    let authorization_expires_at = authorization.approval.token.expires_at.to_rfc3339();

    Ok(SpendHttpResponse {
        operation_key: authorization.approval.operation_key.clone(),
        account_id: authorization.account_pub_id,
        agent_id: authorization.agent_pub_id,
        decision_id: authorization.approval.evaluation.decision_id.to_string(),
        decision: effect_name(authorization.approval.evaluation.evaluation.decision).to_string(),
        reasons: authorization.approval.evaluation.evaluation.reasons,
        auth_token_id: Some(auth_token_id),
        workload_profile,
        authorization_expires_at: Some(authorization_expires_at),
        budget_hold: Some(budget_hold_state_response(
            authorization.approval.budget_reservation.hold,
            authorization.approval.budget_reservation.balance,
        )),
        payment: None,
    })
}

fn spend(body: String, state: &ServerState) -> Result<SpendHttpResponse> {
    let request: SpendHttpRequest = serde_json::from_str(&body)?;
    let authorization = match evaluate_and_reserve_spend(request, state)? {
        SpendAuthorization::Authorized(authorization) => authorization,
        SpendAuthorization::Response(response) => return Ok(response),
    };
    if state
        .spend
        .lock()
        .map_err(|_| anyhow!("spend manager lock poisoned"))?
        .executor_claim_for_operation(
            &authorization.approval.agent_id,
            &authorization.approval.operation_key,
        )
        .is_some()
    {
        return Err(anyhow!(
            "job is already bound to the external executor workflow"
        ));
    }
    let owner = owner_metadata_for_user_id(&authorization.approval.user.user_id, state)?;
    let settlement = submit_authorized_payment(&authorization.approval, state)?;
    let budget_hold = Some(budget_hold_response(settlement.budget_update));
    let payment = Some(PaymentHttpResponse {
        payment_id: settlement.payment.payment_id.to_string(),
        owner_user_id: owner.pub_id,
        owner_user_name: owner.display_name,
        account_id: authorization.account_pub_id.clone(),
        status: payment_status_name(settlement.payment.status).to_string(),
        ledger_transaction_id: settlement
            .payment
            .ledger_transaction_id
            .map(|id| id.to_string()),
        rail_reference: settlement.payment.rail_reference,
        failure_reason: settlement.payment.failure_reason,
    });
    let auth_token_id = authorization.approval.auth_token_id();
    let workload_profile = authorization.approval.workload_profile.clone();
    let authorization_expires_at = authorization.approval.token.expires_at.to_rfc3339();

    Ok(SpendHttpResponse {
        operation_key: authorization.approval.operation_key.clone(),
        account_id: authorization.account_pub_id,
        agent_id: authorization.agent_pub_id,
        decision_id: authorization.approval.evaluation.decision_id.to_string(),
        decision: effect_name(authorization.approval.evaluation.evaluation.decision).to_string(),
        reasons: authorization.approval.evaluation.evaluation.reasons,
        auth_token_id: Some(auth_token_id),
        workload_profile,
        authorization_expires_at: Some(authorization_expires_at),
        budget_hold,
        payment,
    })
}

fn validate_executor_spend(body: String, state: &ServerState) -> Result<ExecutorSpendHttpResponse> {
    let request: ExecutorSpendHttpRequest = serde_json::from_str(&body)?;
    let validated = validate_executor_spend_request(request, state)?;
    Ok(executor_spend_response(&validated))
}

fn claim_executor_spend(
    body: String,
    state: &ServerState,
) -> Result<ExecutorSpendClaimHttpResponse> {
    let request: ExecutorSpendClaimHttpRequest = serde_json::from_str(&body)?;
    let resolved = resolve_executor_spend_request(request.spend, state)?;
    let authorization = resolved.payment_validation_request();
    let claim_state = {
        let mut spend_manager = state
            .spend
            .lock()
            .map_err(|_| anyhow!("spend manager lock poisoned"))?;
        let mut budget_manager = state
            .budgets
            .lock()
            .map_err(|_| anyhow!("budget manager lock poisoned"))?;
        let mut governance = state
            .governance
            .lock()
            .map_err(|_| anyhow!("governance store lock poisoned"))?;
        ExecutorClaimService.claim(
            ClaimExecutorSpendRequest {
                authorization,
                operation_key: request.operation_key,
            },
            &mut spend_manager,
            &mut budget_manager,
            &mut *governance,
        )?
    };
    let spend = executor_spend_response(&executor_spend_from_claim_state(&claim_state, state)?);
    executor_claim_response(&claim_state.claim, spend, state)
}

fn get_executor_claim(
    claim_id: Option<&str>,
    state: &ServerState,
) -> Result<ExecutorSpendClaimHttpResponse> {
    let claim_id = claim_id
        .ok_or_else(|| anyhow!("claim status lookup requires claim_id"))?
        .parse::<SpendExecutorClaimId>()
        .with_context(|| "parse executor spend claim_id")?;
    let user = authenticated_user_context(state)?;
    let claim_state = {
        let spend_manager = state
            .spend
            .lock()
            .map_err(|_| anyhow!("spend manager lock poisoned"))?;
        let budget_manager = state
            .budgets
            .lock()
            .map_err(|_| anyhow!("budget manager lock poisoned"))?;
        ExecutorClaimService.get(&claim_id, &user.user_id, &spend_manager, &budget_manager)?
    };
    executor_claim_http_response(&claim_state, state)
}

fn list_executor_claims_requiring_reconciliation(
    state: &ServerState,
) -> Result<ExecutorClaimsHttpResponse> {
    let user = authenticated_user_context(state)?;
    let claims = {
        let spend_manager = state
            .spend
            .lock()
            .map_err(|_| anyhow!("spend manager lock poisoned"))?;
        let budget_manager = state
            .budgets
            .lock()
            .map_err(|_| anyhow!("budget manager lock poisoned"))?;
        ExecutorClaimService.list_requiring_reconciliation(
            &user.user_id,
            Utc::now(),
            &spend_manager,
            &budget_manager,
        )?
    };
    let claims = claims
        .iter()
        .map(|claim| executor_claim_http_response(claim, state))
        .collect::<Result<Vec<_>>>()?;
    Ok(ExecutorClaimsHttpResponse { claims })
}

fn finalize_executor_spend(
    body: String,
    state: &ServerState,
    vendor_billed: bool,
    reconciliation_capability: Option<&str>,
) -> Result<Value> {
    match serde_json::from_str::<ExecutorSpendFinalizationHttpRequest>(&body)? {
        ExecutorSpendFinalizationHttpRequest::Executor(request) if vendor_billed => {
            settle_executor_spend_request(request, state).map(to_json)
        }
        ExecutorSpendFinalizationHttpRequest::Executor(request) => {
            release_executor_spend_request(request, state).map(to_json)
        }
        ExecutorSpendFinalizationHttpRequest::Reconciliation(request) => {
            authenticate_reconciliation_capability(reconciliation_capability, state)?;
            reconcile_executor_claim(request, state, vendor_billed).map(to_json)
        }
    }
}

fn reconcile_executor_claim(
    request: ExecutorClaimReconciliationHttpRequest,
    state: &ServerState,
    vendor_billed: bool,
) -> Result<ExecutorSpendClaimHttpResponse> {
    let user = authenticated_user_context(state)?;
    let claim_id = request
        .claim_id
        .parse::<SpendExecutorClaimId>()
        .with_context(|| "parse executor spend claim_id")?;
    let outcome = if vendor_billed {
        ExecutorClaimReconciliationOutcome::VendorBilled
    } else {
        ExecutorClaimReconciliationOutcome::VendorDidNotBill
    };
    let claim_state = {
        let mut spend_manager = state
            .spend
            .lock()
            .map_err(|_| anyhow!("spend manager lock poisoned"))?;
        let mut budget_manager = state
            .budgets
            .lock()
            .map_err(|_| anyhow!("budget manager lock poisoned"))?;
        let mut governance = state
            .governance
            .lock()
            .map_err(|_| anyhow!("governance store lock poisoned"))?;
        ExecutorClaimService.reconcile(
            ReconcileExecutorClaimRequest {
                claim_id,
                owner_user_id: user.user_id,
                provider_reference: request.provider_reference,
                evidence: request.evidence,
                outcome,
                receipt: request.receipt,
            },
            Utc::now(),
            &mut spend_manager,
            &mut budget_manager,
            &mut *governance,
        )?
    };
    executor_claim_http_response(&claim_state, state)
}

fn executor_claim_http_response(
    claim_state: &ExecutorClaimState,
    state: &ServerState,
) -> Result<ExecutorSpendClaimHttpResponse> {
    let validated = executor_spend_from_claim_state(claim_state, state)?;
    executor_claim_response(
        &claim_state.claim,
        executor_spend_response(&validated),
        state,
    )
}

#[cfg(test)]
fn settle_executor_spend(
    body: String,
    state: &ServerState,
) -> Result<ExecutorSpendSettlementHttpResponse> {
    let request: ExecutorSpendFinalizeHttpRequest = serde_json::from_str(&body)?;
    settle_executor_spend_request(request, state)
}

fn settle_executor_spend_request(
    request: ExecutorSpendFinalizeHttpRequest,
    state: &ServerState,
) -> Result<ExecutorSpendSettlementHttpResponse> {
    let user = authenticated_user_context(state)?;
    let receipt = request
        .receipt
        .clone()
        .ok_or_else(|| anyhow!("billed executor settlement requires a provider receipt"))?;
    let finalization_request = executor_claim_validation_request(request, &user, state)?;
    let claim_request = SettleExecutorClaimRequest {
        owner_user_id: finalization_request.owner_user_id,
        agent_id: finalization_request.agent_id,
        operation_key: finalization_request.operation_key,
        receipt,
    };
    let claim_state = {
        let mut spend_manager = state
            .spend
            .lock()
            .map_err(|_| anyhow!("spend manager lock poisoned"))?;
        let mut budget_manager = state
            .budgets
            .lock()
            .map_err(|_| anyhow!("budget manager lock poisoned"))?;
        let mut governance = state
            .governance
            .lock()
            .map_err(|_| anyhow!("governance store lock poisoned"))?;
        ExecutorClaimService.settle(
            claim_request,
            Utc::now(),
            &mut spend_manager,
            &mut budget_manager,
            &mut *governance,
        )?
    };
    let validated = executor_spend_from_claim_state(&claim_state, state)?;
    let settlement_id = claim_state
        .claim
        .settlement_id
        .clone()
        .ok_or_else(|| anyhow!("settled executor claim is missing settlement id"))?;
    let receipt = claim_state
        .settlement_receipt
        .as_ref()
        .ok_or_else(|| anyhow!("settled executor claim is missing provider receipt"))?;

    Ok(ExecutorSpendSettlementHttpResponse {
        operation_key: claim_state.claim.operation_key.clone(),
        settlement_id: settlement_id.to_string(),
        claim_id: claim_state.claim.id.to_string(),
        status: executor_claim_status_name(&claim_state.claim.status).to_string(),
        receipt: ExecutorSpendSettlementReceiptHttpResponse {
            authorized_max_cents: receipt.authorized_max_cents,
            actual_vendor_cost_cents: receipt.receipt.actual_vendor_cost_cents,
            released_amount_cents: receipt.released_amount_cents,
            currency: receipt.currency.to_string(),
            provider_request_id: receipt.receipt.provider_request_id.clone(),
            price_model_snapshot: receipt.receipt.price_model_snapshot.clone(),
            artifact_reference: receipt.receipt.artifact_reference.clone(),
            created_at: receipt.created_at.to_rfc3339(),
        },
        spend: executor_spend_response(&validated),
    })
}

#[cfg(test)]
fn release_executor_spend(
    body: String,
    state: &ServerState,
) -> Result<ExecutorSpendClaimHttpResponse> {
    let request: ExecutorSpendFinalizeHttpRequest = serde_json::from_str(&body)?;
    release_executor_spend_request(request, state)
}

fn release_executor_spend_request(
    request: ExecutorSpendFinalizeHttpRequest,
    state: &ServerState,
) -> Result<ExecutorSpendClaimHttpResponse> {
    if request.receipt.is_some() {
        return Err(anyhow!(
            "unbilled executor release cannot include a provider receipt"
        ));
    }
    let user = authenticated_user_context(state)?;
    let claim_request = executor_claim_validation_request(request, &user, state)?;
    let claim_state = {
        let mut spend_manager = state
            .spend
            .lock()
            .map_err(|_| anyhow!("spend manager lock poisoned"))?;
        let mut budget_manager = state
            .budgets
            .lock()
            .map_err(|_| anyhow!("budget manager lock poisoned"))?;
        let mut governance = state
            .governance
            .lock()
            .map_err(|_| anyhow!("governance store lock poisoned"))?;
        ExecutorClaimService.release(
            claim_request,
            Utc::now(),
            &mut spend_manager,
            &mut budget_manager,
            &mut *governance,
        )?
    };
    executor_claim_http_response(&claim_state, state)
}

struct ValidatedExecutorSpend {
    operation_key: String,
    request: ExecutorSpendHttpRequest,
    account_pub_id: String,
    agent_pub_id: String,
    token_id: SpendAuthTokenId,
    validation: hubu_core::spend::ValidatedSpendAuthorization,
    budget_hold: BudgetHold,
    budget_balance: hubu_core::budget::BudgetBalance,
}

struct ResolvedExecutorSpend {
    request: ExecutorSpendHttpRequest,
    account_pub_id: String,
    agent_pub_id: String,
    token_id: SpendAuthTokenId,
    owner_user_id: UserId,
    agent_id: AgentId,
    agent_account_id: hubu_common::ids::AgentAccountId,
}

impl ResolvedExecutorSpend {
    fn payment_validation_request(&self) -> SpendPaymentValidationRequest {
        SpendPaymentValidationRequest {
            spend_auth_token_id: self.token_id.clone(),
            owner_user_id: self.owner_user_id.clone(),
            agent_id: self.agent_id.clone(),
            agent_account_id: self.agent_account_id.clone(),
            amount_cents: self.request.amount_cents,
            currency: Currency::Usd,
            merchant: self.request.merchant.clone(),
            task_id: self.request.task_id.clone(),
        }
    }

    fn into_validated(
        self,
        operation_key: String,
        validation: hubu_core::spend::ValidatedSpendAuthorization,
        budget_hold: BudgetHold,
        budget_balance: hubu_core::budget::BudgetBalance,
    ) -> ValidatedExecutorSpend {
        ValidatedExecutorSpend {
            operation_key,
            request: self.request,
            account_pub_id: self.account_pub_id,
            agent_pub_id: self.agent_pub_id,
            token_id: self.token_id,
            validation,
            budget_hold,
            budget_balance,
        }
    }
}

fn resolve_executor_spend_request(
    request: ExecutorSpendHttpRequest,
    state: &ServerState,
) -> Result<ResolvedExecutorSpend> {
    if request.amount_cents <= 0 {
        return Err(anyhow!("executor spend amount must be positive"));
    }
    reconcile_expired_budget_holds(state)?;

    let user = authenticated_user_context(state)?;
    let spend_request = SpendHttpRequest {
        operation_key: None,
        agent_id: request.agent_id.clone(),
        account_id: request.account_id.clone(),
        amount_cents: request.amount_cents,
        reason: request.task_id.clone().unwrap_or_default(),
        merchant: request.merchant.clone(),
        workload_profile: None,
    };
    let account = resolve_agent_account_for_spend(&spend_request, &user, state)?;
    let account_pub_id = account.pub_id.clone();
    let agent_id = account.agent_id.clone();
    let agent_pub_id = registration_agent_pub_id(&agent_id, state)?;
    let token_id: SpendAuthTokenId = request
        .spend_auth_token_id
        .parse()
        .with_context(|| "parse spend_auth_token_id")?;

    Ok(ResolvedExecutorSpend {
        request,
        account_pub_id,
        agent_pub_id,
        token_id,
        owner_user_id: user.user_id,
        agent_id,
        agent_account_id: account.id,
    })
}

fn validate_executor_spend_request(
    request: ExecutorSpendHttpRequest,
    state: &ServerState,
) -> Result<ValidatedExecutorSpend> {
    let resolved = resolve_executor_spend_request(request, state)?;

    let validation = state
        .spend
        .lock()
        .map_err(|_| anyhow!("spend manager lock poisoned"))?
        .validate_auth_token_for_payment(&resolved.payment_validation_request())?;
    let operation_key = state
        .spend
        .lock()
        .map_err(|_| anyhow!("spend manager lock poisoned"))?
        .decision_record(&validation.spend_decision_id)
        .ok_or_else(|| anyhow!("spend decision is missing"))?
        .operation_key;

    let (budget_hold, budget_balance) = {
        let budgets = state
            .budgets
            .lock()
            .map_err(|_| anyhow!("budget manager lock poisoned"))?;
        let budget_hold = budgets
            .get_budget_hold_by_spend_decision(&validation.spend_decision_id)
            .ok_or_else(|| anyhow!("spend authorization does not have a budget hold"))?;
        if !budgets
            .get_budget_by_id(&budget_hold.budget_id)
            .is_some_and(|budget| budget.budget.agent_id == resolved.agent_id)
        {
            return Err(anyhow!(
                "spend authorization hold does not belong to the authorized agent"
            ));
        }
        if !matches!(budget_hold.status, BudgetHoldStatus::Frozen) {
            return Err(anyhow!("spend authorization budget hold is not frozen"));
        }
        let budget_balance = budgets
            .get_budget_balance(&budget_hold.budget_id)
            .ok_or_else(|| anyhow!("spend authorization budget balance is missing"))?;
        (budget_hold, budget_balance)
    };

    log_event(
        "info",
        "executor_spend_validated",
        json!({
            "spend_auth_token_id": resolved.token_id.to_string(),
            "decision_id": validation.spend_decision_id.to_string(),
            "hold_id": budget_hold.id.to_string(),
            "account_pub_id": resolved.account_pub_id,
            "agent_pub_id": resolved.agent_pub_id,
            "amount_cents": resolved.request.amount_cents,
            "merchant": resolved.request.merchant.clone(),
            "task_id": resolved.request.task_id.clone(),
        }),
    );

    Ok(resolved.into_validated(operation_key, validation, budget_hold, budget_balance))
}

fn executor_claim_validation_request(
    request: ExecutorSpendFinalizeHttpRequest,
    user: &UserContext,
    state: &ServerState,
) -> Result<FinalizeExecutorClaimRequest> {
    let operation_key = request.operation_key.trim();
    if operation_key.is_empty() {
        return Err(anyhow!("executor spend operation_key is required"));
    }
    let agent_pub_id = request.agent_id.trim();
    if agent_pub_id.is_empty() {
        return Err(anyhow!("executor spend agent_id is required"));
    }
    Ok(FinalizeExecutorClaimRequest {
        owner_user_id: user.user_id.clone(),
        agent_id: resolve_agent_id_for_user(agent_pub_id, user, state)?,
        operation_key: operation_key.to_string(),
    })
}

fn executor_spend_from_claim_state(
    claim_state: &ExecutorClaimState,
    state: &ServerState,
) -> Result<ValidatedExecutorSpend> {
    let (account, agent_pub_id) = {
        let registration = state
            .registration
            .lock()
            .map_err(|_| anyhow!("registration manager lock poisoned"))?;
        let account = registration
            .account_for_agent(&claim_state.decision.request.agent_id)?
            .ok_or_else(|| anyhow!("executor claim agent account is missing"))?;
        let agent = registration
            .agent_for_id(&claim_state.decision.request.agent_id)?
            .ok_or_else(|| anyhow!("executor claim agent is missing"))?;
        (account, agent.pub_id)
    };
    if account.id != claim_state.decision.request.agent_account_id {
        return Err(anyhow!(
            "executor claim account does not match spend decision"
        ));
    }

    Ok(ValidatedExecutorSpend {
        operation_key: claim_state.decision.operation_key.clone(),
        request: ExecutorSpendHttpRequest {
            spend_auth_token_id: claim_state.token.id.to_string(),
            agent_id: Some(agent_pub_id.clone()),
            account_id: Some(account.pub_id.clone()),
            amount_cents: claim_state.decision.request.amount_cents,
            merchant: claim_state.decision.request.merchant.clone(),
            task_id: claim_state.decision.request.task_id.clone(),
        },
        account_pub_id: account.pub_id,
        agent_pub_id,
        token_id: claim_state.token.id.clone(),
        validation: claim_state.authorization.clone(),
        budget_hold: claim_state.budget_hold.clone(),
        budget_balance: claim_state.budget_balance.clone(),
    })
}

fn executor_claim_response(
    claim: &SpendExecutorClaimRecord,
    spend: ExecutorSpendHttpResponse,
    state: &ServerState,
) -> Result<ExecutorSpendClaimHttpResponse> {
    let reconciled_by_user_id = claim
        .reconciled_by_user_id
        .as_ref()
        .map(|user_id| owner_metadata_for_user_id(user_id, state).map(|owner| owner.pub_id))
        .transpose()?;
    let reconciliation_outcome = if claim.reconciled_at.is_some() {
        Some(match claim.status {
            SpendExecutorClaimStatus::Settled => "vendor_billed".to_string(),
            SpendExecutorClaimStatus::Released => "vendor_did_not_bill".to_string(),
            SpendExecutorClaimStatus::Claimed => {
                return Err(anyhow!("reconciled executor claim is not finalized"));
            }
        })
    } else {
        None
    };
    Ok(ExecutorSpendClaimHttpResponse {
        operation_key: claim.operation_key.clone(),
        claim_id: claim.id.to_string(),
        workload_profile: claim.workload_profile.clone(),
        status: executor_claim_status_name(&claim.status).to_string(),
        claimed_at: claim.claimed_at.to_rfc3339(),
        claim_expires_at: claim.expires_at.to_rfc3339(),
        finalized_at: claim.finalized_at.map(|timestamp| timestamp.to_rfc3339()),
        settlement_id: claim.settlement_id.as_ref().map(ToString::to_string),
        reconciliation_required: matches!(claim.status, SpendExecutorClaimStatus::Claimed)
            && claim.expires_at <= Utc::now(),
        reconciliation_outcome,
        provider_reference: claim.provider_reference.clone(),
        evidence: claim.reconciliation_evidence.clone(),
        reconciled_at: claim.reconciled_at.map(|timestamp| timestamp.to_rfc3339()),
        reconciled_by_user_id,
        spend,
    })
}

fn executor_claim_status_name(status: &SpendExecutorClaimStatus) -> &'static str {
    match status {
        SpendExecutorClaimStatus::Claimed => "claimed",
        SpendExecutorClaimStatus::Settled => "settled",
        SpendExecutorClaimStatus::Released => "released",
    }
}

fn executor_spend_response(validated: &ValidatedExecutorSpend) -> ExecutorSpendHttpResponse {
    executor_spend_response_with_hold(
        validated,
        validated.budget_hold.clone(),
        validated.budget_balance.clone(),
    )
}

fn executor_spend_response_with_hold(
    validated: &ValidatedExecutorSpend,
    hold: BudgetHold,
    balance: hubu_core::budget::BudgetBalance,
) -> ExecutorSpendHttpResponse {
    ExecutorSpendHttpResponse {
        operation_key: validated.operation_key.clone(),
        spend_auth_token_id: validated.token_id.to_string(),
        decision_id: validated.validation.spend_decision_id.to_string(),
        account_id: validated.account_pub_id.clone(),
        agent_id: validated.agent_pub_id.clone(),
        amount_cents: validated.request.amount_cents,
        currency: Currency::Usd.to_string(),
        merchant: validated.request.merchant.clone(),
        task_id: validated.request.task_id.clone(),
        expires_at: validated.validation.expires_at.to_rfc3339(),
        budget_hold: budget_hold_state_response(hold, balance),
    }
}

struct AuthorizedSpend {
    account_pub_id: String,
    agent_pub_id: String,
    approval: ApprovedSpendAuthorization,
}

enum SpendAuthorization {
    Authorized(AuthorizedSpend),
    Response(SpendHttpResponse),
}

fn evaluate_and_reserve_spend(
    mut request: SpendHttpRequest,
    state: &ServerState,
) -> Result<SpendAuthorization> {
    reconcile_expired_budget_holds(state)?;

    let user = authenticated_user_context(state)?;
    let operation_key = request
        .operation_key
        .take()
        .map(|operation_key| operation_key.trim().to_string())
        .filter(|operation_key| !operation_key.is_empty())
        .ok_or_else(|| anyhow!("spend operation_key is required"))?;
    let account = resolve_agent_account_for_spend(&request, &user, state)?;
    let account_pub_id = account.pub_id.clone();
    let agent_id = account.agent_id.clone();
    let agent_pub_id = registration_agent_pub_id(&agent_id, state)?;
    log_event(
        "info",
        "spend_request_received",
        json!({
            "operation_key": operation_key,
            "agent_pub_id": agent_pub_id,
            "account_pub_id": account_pub_id,
            "user_id": user.user_id.to_string(),
            "amount_cents": request.amount_cents,
            "currency": Currency::Usd.to_string(),
            "merchant": request.merchant.clone(),
            "reason": request.reason.clone(),
        }),
    );
    let policy = policy_for_spend(state, &user.user_id, &agent_id)?
        .ok_or_else(|| anyhow!("no policy found for current user"))?;

    let outcome = {
        let mut spend_manager = state
            .spend
            .lock()
            .map_err(|_| anyhow!("spend manager lock poisoned"))?;
        let mut budget_manager = state
            .budgets
            .lock()
            .map_err(|_| anyhow!("budget manager lock poisoned"))?;
        let mut governance = state
            .governance
            .lock()
            .map_err(|_| anyhow!("governance store lock poisoned"))?;
        let workload_profile = request.workload_profile.clone().unwrap_or_else(|| {
            spend_manager
                .workload_profile_for_operation(&agent_id, &operation_key)
                .unwrap_or_else(|| state.spend_timing.default_profile.clone())
        });
        SpendApprovalService.authorize(
            AuthorizeSpendRequest {
                operation_key: operation_key.clone(),
                user: user.clone(),
                agent_id: agent_id.clone(),
                agent_account_id: account.id,
                amount_cents: request.amount_cents,
                currency: Currency::Usd,
                merchant: request.merchant.clone(),
                task_id: Some(request.reason.clone()),
                workload_profile,
            },
            &policy,
            &mut spend_manager,
            &mut budget_manager,
            &mut *governance,
        )?
    };

    match outcome {
        SpendAuthorizationOutcome::Approved(approval) => {
            log_spend_policy_evaluated(
                &agent_pub_id,
                &agent_id,
                &approval.user.user_id,
                &approval.evaluation,
                true,
            );
            Ok(SpendAuthorization::Authorized(AuthorizedSpend {
                account_pub_id,
                agent_pub_id,
                approval,
            }))
        }
        SpendAuthorizationOutcome::Rejected(rejection) => {
            log_spend_policy_evaluated(
                &agent_pub_id,
                &agent_id,
                &rejection.user.user_id,
                &rejection.evaluation,
                false,
            );
            Ok(SpendAuthorization::Response(spend_rejection_response(
                rejection,
                account_pub_id,
                agent_pub_id,
            )))
        }
    }
}

fn log_spend_policy_evaluated(
    agent_pub_id: &str,
    agent_id: &AgentId,
    user_id: &UserId,
    evaluation: &hubu_core::spend::SpendEvaluationResponse,
    auth_token_returned: bool,
) {
    log_event(
        "info",
        "spend_policy_evaluated",
        json!({
            "agent_pub_id": agent_pub_id,
            "agent_id": agent_id.to_string(),
            "user_id": user_id.to_string(),
            "decision_id": evaluation.decision_id.to_string(),
            "operation_key": evaluation.operation_key,
            "decision": effect_name(evaluation.evaluation.decision),
            "policy_id": evaluation.evaluation.policy_id,
            "policy_version": evaluation.evaluation.policy_version,
            "auth_token_issued": evaluation.auth_token.is_some(),
            "auth_token_returned": auth_token_returned,
        }),
    );
}

fn spend_rejection_response(
    rejection: RejectedSpendAuthorization,
    account_pub_id: String,
    agent_pub_id: String,
) -> SpendHttpResponse {
    SpendHttpResponse {
        operation_key: rejection.operation_key,
        account_id: account_pub_id,
        agent_id: agent_pub_id,
        decision_id: rejection.evaluation.decision_id.to_string(),
        decision: effect_name(rejection.decision).to_string(),
        reasons: rejection.reasons,
        auth_token_id: None,
        workload_profile: rejection.workload_profile,
        authorization_expires_at: None,
        budget_hold: None,
        payment: None,
    }
}

fn submit_authorized_payment(
    authorization: &ApprovedSpendAuthorization,
    state: &ServerState,
) -> Result<hubu_core::app::SpendPaymentSettlement> {
    let spend = Arc::clone(&state.spend);
    let mut payment_manager = state
        .payments
        .lock()
        .map_err(|_| anyhow!("payment manager lock poisoned"))?;
    let mut payment_attempts = state
        .payment_attempts
        .lock()
        .map_err(|_| anyhow!("payment attempt store lock poisoned"))?;
    let mut budget_manager = state
        .budgets
        .lock()
        .map_err(|_| anyhow!("budget manager lock poisoned"))?;
    let mut governance = state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?;

    Ok(SpendApprovalService.submit_payment(
        authorization,
        SpendPaymentSpec {
            idempotency_key: format!(
                "{}:{}",
                authorization.evaluation.decision_id,
                authorization.task_id.as_deref().unwrap_or_default()
            ),
            rail: PaymentRailKind::FiatMock,
            destination: PaymentDestination::FiatAccount {
                account_ref: "local-merchant-account".to_string(),
            },
            memo: Some("Hubu mock payment".to_string()),
            failed_payment_hold_policy: FailedPaymentHoldPolicy::Release,
        },
        &mut payment_manager,
        &mut *payment_attempts,
        &mut budget_manager,
        &mut *governance,
        move |token_id| {
            spend
                .lock()
                .map_err(|_| SpendApprovalError::UsedSpendAuthTokenMissing)?
                .auth_token_record(token_id)
                .ok_or(SpendApprovalError::UsedSpendAuthTokenMissing)
        },
    )?)
}

fn policy_for_spend(
    state: &ServerState,
    user_id: &UserId,
    agent_id: &AgentId,
) -> Result<Option<Policy>> {
    let policies = state
        .policies
        .lock()
        .map_err(|_| anyhow!("policy store lock poisoned"))?;
    Ok(policies
        .get(&(
            user_id.clone(),
            PolicyAssignmentScope::AgentOverride(agent_id.clone()),
        ))
        .or_else(|| policies.get(&(user_id.clone(), PolicyAssignmentScope::UserDefault)))
        .cloned())
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

fn budget_recurrence(recurrence: BudgetRecurrenceHttp) -> BudgetRecurrence {
    match recurrence {
        BudgetRecurrenceHttp::Daily => BudgetRecurrence::Daily,
        BudgetRecurrenceHttp::Monthly => BudgetRecurrence::Monthly,
        BudgetRecurrenceHttp::Yearly => BudgetRecurrence::Yearly,
    }
}

fn budget_series_periods(
    starting_at: DateTime<Utc>,
    recurrence: BudgetRecurrence,
    period_count: usize,
) -> Result<Vec<TimePeriod>> {
    let mut periods = Vec::with_capacity(period_count);
    let mut cursor = starting_at;
    for _ in 0..period_count {
        let ending_before = next_budget_period_boundary(cursor, recurrence)?;
        periods.push(
            TimePeriod::new(cursor, Some(ending_before))
                .map_err(|error| anyhow!("invalid budget period: {error:?}"))?,
        );
        cursor = ending_before;
    }
    Ok(periods)
}

fn next_budget_period_boundary(
    starting_at: DateTime<Utc>,
    recurrence: BudgetRecurrence,
) -> Result<DateTime<Utc>> {
    match recurrence {
        BudgetRecurrence::Daily => starting_at
            .checked_add_signed(Duration::days(1))
            .ok_or_else(|| anyhow!("invalid recurring budget boundary")),
        BudgetRecurrence::Monthly => starting_at
            .checked_add_months(Months::new(1))
            .ok_or_else(|| anyhow!("invalid recurring budget boundary")),
        BudgetRecurrence::Yearly => starting_at
            .checked_add_months(Months::new(12))
            .ok_or_else(|| anyhow!("invalid recurring budget boundary")),
    }
}

fn resolve_agent_account_for_spend(
    request: &SpendHttpRequest,
    user: &UserContext,
    state: &ServerState,
) -> Result<AgentAccount> {
    if request.agent_id.is_some() {
        return Err(anyhow!(
            "spend request must include account_id; agent_id is no longer accepted for spend"
        ));
    }

    let registration = state
        .registration
        .lock()
        .map_err(|_| anyhow!("registration manager lock poisoned"))?;

    let account_pub_id = request
        .account_id
        .as_deref()
        .ok_or_else(|| anyhow!("spend request must include account_id"))?;
    let account = registration
        .account_for_pub_id(account_pub_id)?
        .ok_or_else(|| anyhow!("unknown public account id {account_pub_id}"))?;

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
            budget_id: public_budget_id(&response.hold.budget_id),
            status: "settled".to_string(),
            amount_cents: response.hold.amount_cents,
            consumed_amount_cents: response.balance.consumed_amount_cents,
            frozen_amount_cents: response.balance.frozen_amount_cents,
            remaining_amount_cents: response.balance.remaining_amount_cents,
        },
        BudgetHoldUpdate::Released(response) => BudgetHoldHttpResponse {
            hold_id: response.hold.id.to_string(),
            budget_id: public_budget_id(&response.hold.budget_id),
            status: "released".to_string(),
            amount_cents: response.hold.amount_cents,
            consumed_amount_cents: response.balance.consumed_amount_cents,
            frozen_amount_cents: response.balance.frozen_amount_cents,
            remaining_amount_cents: response.balance.remaining_amount_cents,
        },
        BudgetHoldUpdate::Frozen(response) => frozen_budget_hold_response(response),
    }
}

fn frozen_budget_hold_response(response: ReserveBudgetResponse) -> BudgetHoldHttpResponse {
    BudgetHoldHttpResponse {
        hold_id: response.hold.id.to_string(),
        budget_id: public_budget_id(&response.hold.budget_id),
        status: "frozen".to_string(),
        amount_cents: response.hold.amount_cents,
        consumed_amount_cents: response.balance.consumed_amount_cents,
        frozen_amount_cents: response.balance.frozen_amount_cents,
        remaining_amount_cents: response.balance.remaining_amount_cents,
    }
}

fn budget_hold_state_response(
    hold: BudgetHold,
    balance: hubu_core::budget::BudgetBalance,
) -> BudgetHoldHttpResponse {
    BudgetHoldHttpResponse {
        hold_id: hold.id.to_string(),
        budget_id: public_budget_id(&hold.budget_id),
        status: budget_hold_status_name(&hold.status).to_string(),
        amount_cents: hold.amount_cents,
        consumed_amount_cents: balance.consumed_amount_cents,
        frozen_amount_cents: balance.frozen_amount_cents,
        remaining_amount_cents: balance.remaining_amount_cents,
    }
}

fn budget_response(budget: BudgetWithBalance, state: &ServerState) -> Result<BudgetHttpResponse> {
    Ok(BudgetHttpResponse {
        budget_id: public_budget_id(&budget.budget.id),
        agent_id: registration_agent_pub_id(&budget.budget.agent_id, state)?,
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
    })
}

fn spending_target_response(
    target: SpendingTarget,
    state: &ServerState,
) -> Result<SpendingTargetHttpResponse> {
    let allocated_amount_cents = max_concurrent_allocated_amount(
        &budgets_for_user_id(&target.owner_user_id, state)?,
        &target,
    );
    let exceeded_by_cents = (allocated_amount_cents - target.target_amount_cents).max(0);
    Ok(SpendingTargetHttpResponse {
        target_id: public_spending_target_id(&target.id),
        target_amount_cents: target.target_amount_cents,
        allocated_amount_cents,
        exceeded_by_cents,
        is_exceeded: exceeded_by_cents > 0,
        currency: target.currency.to_string(),
        starting_at: target.period.starting_at.to_rfc3339(),
        ending_before: target
            .period
            .ending_before
            .map(|ending_before| ending_before.to_rfc3339()),
        status: spending_target_status_name(&target).to_string(),
    })
}

fn spending_target_warnings_for_periods(
    user: &UserContext,
    periods: &[TimePeriod],
    currency: Currency,
    state: &ServerState,
) -> Result<Vec<SpendingTargetWarningHttpResponse>> {
    let targets = {
        let manager = state
            .spending_targets
            .lock()
            .map_err(|_| anyhow!("spending target manager lock poisoned"))?;
        manager.get_targets_by_user_id(&user.user_id)
    };
    let budgets = budgets_for_user(user, state)?;
    Ok(targets
        .into_iter()
        .filter(|target| {
            target.status == SpendingTargetStatus::Active
                && target.currency == currency
                && periods
                    .iter()
                    .any(|period| periods_overlap(&target.period, period))
        })
        .filter_map(|target| {
            let allocated_amount_cents = max_concurrent_allocated_amount(&budgets, &target);
            let exceeded_by_cents =
                (allocated_amount_cents - target.target_amount_cents).max(0);
            (exceeded_by_cents > 0).then(|| SpendingTargetWarningHttpResponse {
                target_id: public_spending_target_id(&target.id),
                target_amount_cents: target.target_amount_cents,
                allocated_amount_cents,
                exceeded_by_cents,
                message: format!(
                    "agent budget allocations exceed the advisory spending target by {exceeded_by_cents} cents; budget creation was not blocked"
                ),
            })
        })
        .collect())
}

fn budgets_for_user_id(user_id: &UserId, state: &ServerState) -> Result<Vec<BudgetWithBalance>> {
    budgets_for_user(&UserContext::new(user_id.clone()), state)
}

fn max_concurrent_allocated_amount(budgets: &[BudgetWithBalance], target: &SpendingTarget) -> i64 {
    let mut changes = BTreeMap::<DateTime<Utc>, i64>::new();
    for budget in budgets.iter().filter(|budget| {
        budget.budget.currency == target.currency
            && !matches!(budget.budget.status, BudgetStatus::Revoked)
            && periods_overlap(&budget.budget.period, &target.period)
    }) {
        let starting_at = budget
            .budget
            .period
            .starting_at
            .max(target.period.starting_at);
        let ending_before = match (
            budget.budget.period.ending_before,
            target.period.ending_before,
        ) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(left), None) => Some(left),
            (None, Some(right)) => Some(right),
            (None, None) => None,
        };
        if ending_before.is_some_and(|ending_before| ending_before <= starting_at) {
            continue;
        }
        *changes.entry(starting_at).or_default() += budget.budget.amount_limit_cents;
        if let Some(ending_before) = ending_before {
            *changes.entry(ending_before).or_default() -= budget.budget.amount_limit_cents;
        }
    }

    let mut allocated = 0_i64;
    let mut maximum = 0_i64;
    for change in changes.values() {
        allocated += change;
        maximum = maximum.max(allocated);
    }
    maximum
}

fn spending_target_status_name(target: &SpendingTarget) -> &'static str {
    if target.status == SpendingTargetStatus::Revoked {
        "revoked"
    } else if target
        .period
        .ending_before
        .is_some_and(|ending_before| ending_before <= Utc::now())
    {
        "expired"
    } else if target.period.starting_at > Utc::now() {
        "scheduled"
    } else {
        "active"
    }
}

fn public_budget_id(budget_id: &BudgetId) -> String {
    format!("bgt_{}", budget_id.public_suffix())
}

fn public_spending_target_id(target_id: &SpendingTargetId) -> String {
    format!("tgt_{}", target_id.public_suffix())
}

fn required_budget_agent_id(
    agent_pub_id: Option<&str>,
    user: &UserContext,
    state: &ServerState,
) -> Result<AgentId> {
    match agent_pub_id {
        Some(agent_pub_id) => resolve_agent_id_for_user(agent_pub_id, user, state),
        None => Err(anyhow!("budget create requires --agent-id")),
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

fn budget_hold_status_name(status: &BudgetHoldStatus) -> &'static str {
    match status {
        BudgetHoldStatus::Frozen => "frozen",
        BudgetHoldStatus::Claimed => "claimed",
        BudgetHoldStatus::Settled => "settled",
        BudgetHoldStatus::Released => "released",
        BudgetHoldStatus::Expired => "expired",
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

fn authenticated_user_context(state: &ServerState) -> Result<UserContext> {
    Ok(UserContext::new(state.auth.owner_user_id()?))
}

fn authenticated_user(state: &ServerState) -> Result<User> {
    let owner_user_id = state.auth.owner_user_id()?;
    let users = state
        .users
        .lock()
        .map_err(|_| anyhow!("user manager lock poisoned"))?;
    users
        .user_for_id(&owner_user_id)?
        .ok_or_else(|| anyhow!("authenticated local API owner is missing"))
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
        username: user.username,
        display_name: user.display_name,
    })
}

fn list_ledger(state: &ServerState) -> Result<LedgerHttpResponse> {
    let user = authenticated_user_context(state)?;
    let payments = state
        .payments
        .lock()
        .map_err(|_| anyhow!("payment manager lock poisoned"))?;
    let transactions = payments
        .ledger()
        .list_transactions()?
        .into_iter()
        .filter(|transaction| transaction.owner_user_id == user.user_id)
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

fn payment_status_name(status: PaymentStatus) -> &'static str {
    match status {
        PaymentStatus::Succeeded => "succeeded",
        PaymentStatus::Failed => "failed",
    }
}

fn to_json<T: Serialize>(value: T) -> Value {
    serde_json::to_value(value).expect("HTTP response should serialize")
}

struct HttpRequest {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: String,
}

struct HttpResponse {
    status: u16,
    body: Value,
}

struct OwnerHttpMetadata {
    pub_id: String,
    username: Option<String>,
    display_name: String,
}

fn parse_request(raw: &str) -> Result<HttpRequest> {
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow!("invalid HTTP request"))?;
    let mut request_line = head
        .split("\r\n")
        .next()
        .unwrap_or_default()
        .split_whitespace();
    let method = request_line
        .next()
        .ok_or_else(|| anyhow!("missing method"))?
        .to_string();
    let target = request_line.next().ok_or_else(|| anyhow!("missing path"))?;
    let version = request_line
        .next()
        .ok_or_else(|| anyhow!("missing HTTP version"))?;
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(anyhow!("unsupported HTTP version"));
    }
    if request_line.next().is_some() {
        return Err(anyhow!("malformed HTTP request line"));
    }
    let (path, query) = split_path_and_query(target);
    let mut headers = HashMap::new();
    for line in head.split("\r\n").skip(1) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| anyhow!("malformed HTTP header"))?;
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() {
            return Err(anyhow!("empty HTTP header name"));
        }
        if headers.insert(name, value.trim().to_string()).is_some() {
            return Err(anyhow!("duplicate HTTP header"));
        }
    }

    Ok(HttpRequest {
        method,
        path,
        query,
        headers,
        body: body.to_string(),
    })
}

fn split_path_and_query(target: &str) -> (String, HashMap<String, String>) {
    let Some((path, query_text)) = target.split_once('?') else {
        return (target.to_string(), HashMap::new());
    };
    let query = query_text
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (key.to_string(), value.to_string())
        })
        .collect();
    (path.to_string(), query)
}

fn query_flag(request: &HttpRequest, name: &str) -> bool {
    request
        .query
        .get(name)
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes"))
}

fn write_response(stream: &mut TcpStream, response: HttpResponse) -> Result<()> {
    let body = response.body.to_string();
    let status_text = match response.status {
        200 => "OK",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Bad Request",
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
    fn parses_spend_timing_yaml_config() {
        let config = parse_spend_timing_config(
            r#"
default_profile: batch
profiles:
  batch:
    authorization_ttl_seconds: 600
    claim_ttl_seconds: 1800
"#,
            "test-spend-timing.yaml",
        )
        .expect("spend timing YAML should parse");

        assert_eq!(config.default_profile, "batch");
        let batch = config.profiles.get("batch").expect("batch profile");
        assert_eq!(batch.authorization_ttl_seconds, 600);
        assert_eq!(batch.claim_ttl_seconds, 1800);
    }

    #[test]
    fn spend_timing_yaml_rejects_unknown_fields() {
        let error = parse_spend_timing_config(
            r#"
default_profile: batch
profiles:
  batch:
    authorization_ttl_seconds: 600
    claim_ttl_seconds: 1800
    retry_seconds: 10
"#,
            "test-spend-timing.yaml",
        )
        .expect_err("unknown timing fields should fail");

        assert!(error
            .to_string()
            .contains("parse spend timing config `test-spend-timing.yaml`"));
    }

    #[test]
    fn spend_timing_yaml_rejects_missing_fields_and_malformed_yaml() {
        for yaml in [
            "default_profile: batch\nprofiles:\n  batch:\n    authorization_ttl_seconds: 600\n",
            "default_profile: [batch\nprofiles: {}\n",
        ] {
            let error = parse_spend_timing_config(yaml, "test-spend-timing.yaml")
                .expect_err("invalid timing YAML should fail");
            assert!(error
                .to_string()
                .contains("parse spend timing config `test-spend-timing.yaml`"));
        }
    }

    fn read_test_request(reader: &mut impl Read) -> Result<String> {
        read_http_request_with_guard(reader, |_| Ok(()))
    }

    #[test]
    fn reads_exact_declared_body_without_waiting_for_eof() {
        let request =
            b"POST /init HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\n{}trailing";
        let raw = read_test_request(&mut request.as_slice()).expect("request should be framed");

        assert_eq!(
            raw,
            "POST /init HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\n{}"
        );
        assert_eq!(
            parse_request(&raw).expect("request should parse").body,
            "{}"
        );
    }

    #[test]
    fn preserves_body_bytes_read_with_the_headers() {
        let first = b"POST /init HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\n{";
        let second = b"}";
        let mut request = first.as_slice().chain(second.as_slice());

        let raw = read_test_request(&mut request).expect("chunked request should be framed");

        assert_eq!(
            parse_request(&raw).expect("request should parse").body,
            "{}"
        );
    }

    #[test]
    fn checks_the_absolute_deadline_before_every_read() {
        struct OneByteReader<'a>(&'a [u8]);

        impl Read for OneByteReader<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if self.0.is_empty() || buffer.is_empty() {
                    return Ok(0);
                }
                buffer[0] = self.0[0];
                self.0 = &self.0[1..];
                Ok(1)
            }
        }

        let mut request = OneByteReader(b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n");
        let mut reads_allowed = 3;
        let error = read_http_request_with_guard(&mut request, |_| {
            if reads_allowed == 0 {
                return Err(anyhow!("HTTP request read deadline exceeded"));
            }
            reads_allowed -= 1;
            Ok(())
        })
        .expect_err("slow-drip request should exceed the absolute deadline");

        assert_eq!(error.to_string(), "HTTP request read deadline exceeded");
    }

    #[test]
    fn standard_client_receives_response_without_half_close() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
        let address = listener
            .local_addr()
            .expect("listener should have an address");
        let path =
            std::env::temp_dir().join(format!("hubu-api-standard-client-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("server should accept client");
            handle_connection(stream, &state)
        });

        let mut client = TcpStream::connect(address).expect("client should connect");
        client
            .set_read_timeout(Some(StdDuration::from_secs(2)))
            .expect("client timeout should be set");
        client
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .expect("client should send request");
        let mut response = String::new();
        let read_result = client.read_to_string(&mut response);
        drop(client);

        server
            .join()
            .expect("server thread should finish")
            .expect("server should handle request");
        read_result.expect("client should receive response without closing its write side");
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.ends_with("{\"status\":\"ok\"}"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn rejects_incomplete_malformed_and_oversized_requests() {
        let incomplete = b"GET /health HTTP/1.1\r\nHost: localhost\r\n";
        assert!(read_test_request(&mut incomplete.as_slice()).is_err());

        let incomplete_body =
            b"POST /init HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\n{";
        assert!(read_test_request(&mut incomplete_body.as_slice()).is_err());

        let malformed_length =
            b"POST /init HTTP/1.1\r\nHost: localhost\r\nContent-Length: nope\r\n\r\n";
        assert!(read_test_request(&mut malformed_length.as_slice()).is_err());

        let unsupported_transfer_encoding =
            b"POST /init HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n";
        assert!(read_test_request(&mut unsupported_transfer_encoding.as_slice()).is_err());

        let oversized_body = format!(
            "POST /init HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n",
            MAX_HTTP_BODY_BYTES + 1
        );
        assert!(read_test_request(&mut oversized_body.as_bytes()).is_err());

        let mut oversized_headers = b"GET /health HTTP/1.1\r\nX-Large: ".to_vec();
        oversized_headers.resize(oversized_headers.len() + MAX_HTTP_HEADER_BYTES, b'a');
        assert!(read_test_request(&mut oversized_headers.as_slice()).is_err());

        let malformed_header = "GET /health HTTP/1.1\r\nnot-a-header\r\n\r\n";
        assert!(parse_request(malformed_header).is_err());
    }

    fn settlement_receipt_json(actual_vendor_cost_cents: i64) -> Value {
        json!({
            "actual_vendor_cost_cents": actual_vendor_cost_cents,
            "provider_request_id": "provider-request-123",
            "price_model_snapshot": {
                "provider": "example-image-provider",
                "model": "image-model-v1",
                "unit_price_cents": actual_vendor_cost_cents,
                "pricing_unit": "image",
                "currency": "usd",
            },
            "artifact_reference": "artifact://hubu-logo.png",
        })
    }

    fn public_request(method: &str, path: &str) -> HttpRequest {
        let (path, query) = split_path_and_query(path);
        HttpRequest {
            method: method.to_string(),
            path,
            query,
            headers: HashMap::new(),
            body: String::new(),
        }
    }

    fn authenticated_json_request(path: &str, body: Value) -> HttpRequest {
        let (path, query) = split_path_and_query(path);
        let mut headers = HashMap::new();
        headers.insert("host".to_string(), "127.0.0.1:8787".to_string());
        headers.insert("content-type".to_string(), "application/json".to_string());
        headers.insert(
            "authorization".to_string(),
            format!("Bearer {TEST_AUTH_TOKEN}"),
        );
        HttpRequest {
            method: "POST".to_string(),
            path,
            query,
            headers,
            body: body.to_string(),
        }
    }

    fn authenticated_get_request(path: &str) -> HttpRequest {
        let (path, query) = split_path_and_query(path);
        let mut headers = HashMap::new();
        headers.insert("host".to_string(), "127.0.0.1:8787".to_string());
        headers.insert(
            "authorization".to_string(),
            format!("Bearer {TEST_AUTH_TOKEN}"),
        );
        HttpRequest {
            method: "GET".to_string(),
            path,
            query,
            headers,
            body: String::new(),
        }
    }

    fn set_test_spending_target(
        state: &ServerState,
        amount_cents: i64,
    ) -> SpendingTargetHttpResponse {
        set_spending_target(
            json!({
                "amount_cents": amount_cents,
            })
            .to_string(),
            state,
        )
        .expect("spending target should be set")
        .target
    }

    fn create_test_agent_budget(
        state: &ServerState,
        agent_pub_id: &str,
        amount_cents: i64,
    ) -> CreateBudgetHttpResponse {
        create_budget(
            json!({
                "agent_id": agent_pub_id,
                "amount_cents": amount_cents,
            })
            .to_string(),
            state,
        )
        .expect("agent budget should be created")
    }

    #[test]
    fn registration_guidance_is_available_for_agents() {
        let path = std::env::temp_dir().join(format!("hubu-api-guidance-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");

        for path in [
            "/registration/guidance",
            "/.well-known/hubu-agent-registration.json",
        ] {
            let response = route(public_request("GET", path), &state);

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
    fn version_metadata_is_public_and_complete() {
        let path =
            std::env::temp_dir().join(format!("hubu-api-version-info-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");

        let response = route(public_request("GET", "/version"), &state);

        assert_eq!(response.status, 200);
        assert_eq!(
            response.body["product_version"],
            build_info().product_version
        );
        assert_eq!(response.body["executor_contract"], "hubu-spend-executor-v4");
        assert!(response.body["source_commit"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn user_list_returns_registered_human_users() {
        let path =
            std::env::temp_dir().join(format!("hubu-api-user-list-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        let alice_response = route(
            authenticated_json_request(
                "/init",
                json!({
                    "username": "alice-example",
                    "display_name": "Alice Example",
                    "email": "alice@example.com",
                }),
            ),
            &state,
        );
        assert_eq!(alice_response.status, 200);
        route(
            authenticated_json_request(
                "/init",
                json!({
                    "username": "bob-example",
                    "display_name": "Bob Example",
                    "email": "bob@example.com",
                }),
            ),
            &state,
        );

        let list_response = route(authenticated_get_request("/users"), &state);

        assert_eq!(list_response.status, 200);
        let users = list_response.body["users"]
            .as_array()
            .expect("users should be an array");
        assert_eq!(
            users
                .iter()
                .filter(|user| user["current"].as_bool() == Some(true))
                .count(),
            1
        );
        let alice = users
            .iter()
            .find(|user| user["username"] == "alice-example")
            .expect("alice should be listed");
        let bob = users
            .iter()
            .find(|user| user["username"] == "bob-example")
            .expect("bob should be listed");
        assert_eq!(alice["user_id"], alice_response.body["user_id"]);
        assert_eq!(alice["current"], false);
        assert_eq!(bob["display_name"], "Bob Example");
        assert_eq!(bob["current"], true);
        assert!(bob["created_at"].as_str().is_some());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn duplicate_user_email_returns_helpful_error() {
        let path =
            std::env::temp_dir().join(format!("hubu-api-duplicate-email-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        let first = route(
            authenticated_json_request(
                "/init",
                json!({
                    "username": "alice",
                    "display_name": "Alice Example",
                    "email": "alice@example.com",
                }),
            ),
            &state,
        );
        assert_eq!(first.status, 200);

        let duplicate = route(
            authenticated_json_request(
                "/init",
                json!({
                    "username": "alice-2",
                    "display_name": "Alice Duplicate",
                    "email": "Alice@Example.com",
                }),
            ),
            &state,
        );

        assert_eq!(duplicate.status, 400);
        assert!(duplicate.body["error"]
            .as_str()
            .expect("error should be a string")
            .contains("email `alice@example.com` is already registered"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn protected_routes_require_local_bearer_authorization() {
        let path = std::env::temp_dir().join(format!("hubu-api-auth-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");

        let response = route(public_request("GET", "/user"), &state);

        assert_eq!(response.status, 401);
        assert!(response.body["error"]
            .as_str()
            .expect("error should be a string")
            .contains("missing authorization bearer token"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn protected_routes_reject_browser_origin_requests() {
        let path = std::env::temp_dir().join(format!("hubu-api-origin-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        let mut request = authenticated_get_request("/user");
        request
            .headers
            .insert("origin".to_string(), "https://example.test".to_string());

        let response = route(request, &state);

        assert_eq!(response.status, 401);
        assert!(response.body["error"]
            .as_str()
            .expect("error should be a string")
            .contains("browser-origin requests"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn registration_envelope_registers_agent() {
        let path = std::env::temp_dir().join(format!("hubu-api-envelope-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        let user = init(
            json!({
                "username": "alice-example",
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
    fn registration_envelope_rejects_non_current_owner_user_id() {
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
            .contains("identity payload owner.pub_id must match the current Hubu user"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn registration_envelope_cannot_select_a_different_existing_user() {
        let path =
            std::env::temp_dir().join(format!("hubu-api-current-owner-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        let alice = init(
            json!({
                "display_name": "Alice Example",
                "email": "alice@example.com",
            })
            .to_string(),
            &state,
        )
        .expect("init should create first user");
        init(
            json!({
                "display_name": "Bob Example",
                "email": "bob@example.com",
            })
            .to_string(),
            &state,
        )
        .expect("init should create and select second user");
        let envelope = simple_registration_envelope("protocol-agent", "dev", &alice.user_id);

        let error = register_agent(
            serde_json::to_string(&envelope).expect("envelope should serialize"),
            &state,
        )
        .expect_err("registration should not be able to choose another user");

        assert!(error
            .to_string()
            .contains("identity payload owner.pub_id must match the current Hubu user"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn duplicate_agent_registration_returns_concise_error() {
        let path =
            std::env::temp_dir().join(format!("hubu-api-duplicate-agent-{}.sqlite", UserId::new()));
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

        register_agent(
            serde_json::to_string(&envelope).expect("envelope should serialize"),
            &state,
        )
        .expect("first registration should succeed");
        let error = register_agent(
            serde_json::to_string(&envelope).expect("envelope should serialize"),
            &state,
        )
        .expect_err("duplicate registration should fail");

        assert_eq!(
            error.to_string(),
            "agent is already registered for this owner"
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn agent_list_defaults_to_current_user_and_all_flag_expands_scope() {
        let path = std::env::temp_dir().join(format!(
            "hubu-api-agent-list-scope-{}.sqlite",
            UserId::new()
        ));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        init(
            json!({
                "username": "alice-example",
                "display_name": "Alice Example",
                "email": "alice@example.com",
            })
            .to_string(),
            &state,
        )
        .expect("init should create alice");
        let agent = register_agent(
            json!({
                "name": "alice-agent",
                "version": "v1",
            })
            .to_string(),
            &state,
        )
        .expect("alice agent should register");
        init(
            json!({
                "username": "bob-example",
                "display_name": "Bob Example",
                "email": "bob@example.com",
            })
            .to_string(),
            &state,
        )
        .expect("init should create and select bob");

        let current_only = route(authenticated_get_request("/agents"), &state);
        assert_eq!(current_only.status, 200);
        assert!(current_only.body["agents"]
            .as_array()
            .expect("agents should be an array")
            .is_empty());

        let all_agents = route(authenticated_get_request("/agents?all=true"), &state);
        assert_eq!(all_agents.status, 200);
        let agents = all_agents.body["agents"]
            .as_array()
            .expect("agents should be an array");
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0]["agent_id"], agent.agent_id);
        assert_eq!(agents[0]["owner_username"], "alice-example");
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
                "daily_limit_cents": 5_000,
            })
            .to_string(),
            &state,
        )
        .expect("policy should be added under initialized user");

        create_test_agent_budget(&state, &agent.agent_id, 10_000);

        let spend = spend(
            json!({
                "operation_key": "owner-metadata-job",
                "account_id": agent.account_id,
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
    fn agent_budget_is_listed_and_used_for_spend() {
        let path =
            std::env::temp_dir().join(format!("hubu-api-agent-budget-{}.sqlite", UserId::new()));
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
                "name": "agent-budget-agent",
                "version": "v1",
            })
            .to_string(),
            &state,
        )
        .expect("agent should register under initialized user");
        let agent_pub_id = agent.agent_id.clone();

        add_policy(
            json!({
                "daily_limit_cents": 5_000,
            })
            .to_string(),
            &state,
        )
        .expect("policy should be added under initialized user");

        let agent_budget = create_budget(
            json!({
                "agent_id": agent_pub_id.clone(),
                "amount_cents": 3_000,
            })
            .to_string(),
            &state,
        )
        .expect("agent budget should be created");
        assert!(agent_budget.budget.budget_id.starts_with("bgt_"));
        assert_eq!(agent_budget.budget.agent_id, agent_pub_id);

        let budgets = list_budgets(&state, false).expect("budgets should list");
        assert_eq!(budgets.budgets.len(), 1);
        assert!(budgets
            .budgets
            .iter()
            .any(|budget| budget.agent_id == agent_pub_id));

        let spend = spend(
            json!({
                "operation_key": "agent-budget-job",
                "account_id": agent.account_id,
                "amount_cents": 2_500,
                "reason": "agent budget purchase",
                "merchant": "Acme Cafe",
            })
            .to_string(),
            &state,
        )
        .expect("spend should use the agent budget");
        let budget_hold = spend
            .budget_hold
            .expect("allowed spend should reserve budget");
        assert!(budget_hold.budget_id.starts_with("bgt_"));
        assert_eq!(budget_hold.consumed_amount_cents, 2_500);
        assert_eq!(budget_hold.remaining_amount_cents, 500);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn budget_revoke_uses_public_budget_id() {
        let path =
            std::env::temp_dir().join(format!("hubu-api-budget-revoke-{}.sqlite", UserId::new()));
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
                "name": "budget-revoke-agent",
                "version": "v1",
            })
            .to_string(),
            &state,
        )
        .expect("agent should register");

        let budget = create_budget(
            json!({
                "agent_id": agent.agent_id,
                "amount_cents": 10_000,
            })
            .to_string(),
            &state,
        )
        .expect("budget should be created");
        assert!(budget.budget.budget_id.starts_with("bgt_"));

        let revoked = revoke_budget(
            json!({
                "budget_id": budget.budget.budget_id,
            })
            .to_string(),
            &state,
        )
        .expect("budget should revoke");

        assert_eq!(revoked.budget.status, "revoked");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn budget_replace_revokes_old_budget_and_creates_forward_allowance() {
        let path =
            std::env::temp_dir().join(format!("hubu-api-budget-replace-{}.sqlite", UserId::new()));
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
                "name": "budget-replace-agent",
                "version": "v1",
            })
            .to_string(),
            &state,
        )
        .expect("agent should register");

        let budget = create_budget(
            json!({
                "agent_id": agent.agent_id,
                "amount_cents": 10_000,
            })
            .to_string(),
            &state,
        )
        .expect("budget should be created");
        let original_budget_id = budget.budget.budget_id.clone();

        let replaced = replace_budget(
            json!({
                "budget_id": original_budget_id.clone(),
                "amount_cents": 20_000,
            })
            .to_string(),
            &state,
        )
        .expect("budget should replace");

        assert_eq!(replaced.revoked_budget.budget_id, original_budget_id);
        assert_eq!(replaced.revoked_budget.status, "revoked");
        assert!(replaced.budget.budget_id.starts_with("bgt_"));
        assert_ne!(replaced.budget.budget_id, original_budget_id);
        assert_eq!(replaced.budget.status, "active");
        assert_eq!(replaced.budget.amount_limit_cents, 20_000);
        assert_eq!(replaced.budget.remaining_amount_cents, 20_000);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn budget_list_hides_revoked_budgets_unless_all_is_requested() {
        let path = std::env::temp_dir().join(format!(
            "hubu-api-budget-list-active-{}.sqlite",
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
                "name": "budget-list-agent",
                "version": "v1",
            })
            .to_string(),
            &state,
        )
        .expect("agent should register");

        let budget = create_budget(
            json!({
                "agent_id": agent.agent_id,
                "amount_cents": 10_000,
            })
            .to_string(),
            &state,
        )
        .expect("budget should be created");
        revoke_budget(
            json!({
                "budget_id": budget.budget.budget_id,
            })
            .to_string(),
            &state,
        )
        .expect("budget should revoke");

        let active_only = route(authenticated_get_request("/budgets"), &state);
        assert_eq!(active_only.status, 200);
        assert_eq!(
            active_only.body["budgets"]
                .as_array()
                .expect("budgets should be an array")
                .len(),
            0
        );

        let with_history = route(authenticated_get_request("/budgets?all=true"), &state);
        assert_eq!(with_history.status, 200);
        let budgets = with_history.body["budgets"]
            .as_array()
            .expect("budgets should be an array");
        assert_eq!(budgets.len(), 1);
        assert_eq!(budgets[0]["status"], "revoked");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn budget_creation_warns_when_advisory_spending_target_is_exceeded() {
        let path = std::env::temp_dir().join(format!(
            "hubu-api-agent-budget-hierarchy-{}.sqlite",
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
                "name": "target-warning-agent",
                "version": "v1",
            })
            .to_string(),
            &state,
        )
        .expect("agent should register");

        let target = set_test_spending_target(&state, 1_000);

        let budget = create_budget(
            json!({
                "agent_id": agent.agent_id,
                "amount_cents": 2_000,
            })
            .to_string(),
            &state,
        )
        .expect("advisory target should not block budget creation");

        assert_eq!(budget.budget.amount_limit_cents, 2_000);
        assert_eq!(budget.spending_target_warnings.len(), 1);
        let warning = &budget.spending_target_warnings[0];
        assert_eq!(warning.target_id, target.target_id);
        assert_eq!(warning.target_amount_cents, 1_000);
        assert_eq!(warning.allocated_amount_cents, 2_000);
        assert_eq!(warning.exceeded_by_cents, 1_000);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn spending_target_reports_existing_agent_budget_allocations() {
        let path = std::env::temp_dir().join(format!(
            "hubu-api-user-budget-hierarchy-{}.sqlite",
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
                "name": "existing-budget-target-agent",
                "version": "v1",
            })
            .to_string(),
            &state,
        )
        .expect("agent should register");

        create_budget(
            json!({
                "agent_id": agent.agent_id,
                "amount_cents": 2_000,
            })
            .to_string(),
            &state,
        )
        .expect("agent budget should be created without a spending target");

        let target = set_test_spending_target(&state, 1_000);

        assert_eq!(target.target_amount_cents, 1_000);
        assert_eq!(target.allocated_amount_cents, 2_000);
        assert_eq!(target.exceeded_by_cents, 1_000);
        assert!(target.is_exceeded);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn spending_target_uses_maximum_concurrent_not_cumulative_allocations() {
        let path = std::env::temp_dir().join(format!(
            "hubu-api-concurrent-target-{}.sqlite",
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
        let first_agent = register_agent(
            json!({
                "name": "first-adjacent-budget-agent",
                "version": "v1",
            })
            .to_string(),
            &state,
        )
        .expect("first agent should register");
        let second_agent = register_agent(
            json!({
                "name": "second-adjacent-budget-agent",
                "version": "v1",
            })
            .to_string(),
            &state,
        )
        .expect("second agent should register");
        let boundary = Utc::now() + Duration::hours(1);
        create_budget(
            json!({
                "agent_id": first_agent.agent_id,
                "amount_cents": 2_000,
                "ending_before": boundary.to_rfc3339(),
            })
            .to_string(),
            &state,
        )
        .expect("first budget should create");
        create_budget(
            json!({
                "agent_id": second_agent.agent_id,
                "amount_cents": 2_000,
                "starting_at": boundary.to_rfc3339(),
                "ending_before": (boundary + Duration::hours(1)).to_rfc3339(),
            })
            .to_string(),
            &state,
        )
        .expect("adjacent budget should create");

        let target = set_test_spending_target(&state, 2_500);

        assert_eq!(target.allocated_amount_cents, 2_000);
        assert_eq!(target.exceeded_by_cents, 0);
        assert!(!target.is_exceeded);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn spending_target_is_advisory_and_survives_restart() {
        let path = std::env::temp_dir().join(format!(
            "hubu-api-spending-target-restart-{}.sqlite",
            UserId::new()
        ));
        {
            let state =
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
                    "name": "advisory-target-agent",
                    "version": "v1",
                })
                .to_string(),
                &state,
            )
            .expect("agent should register");
            add_policy(
                json!({
                    "agent_id": agent.agent_id,
                    "daily_limit_cents": 2_000,
                })
                .to_string(),
                &state,
            )
            .expect("policy should be added");

            set_test_spending_target(&state, 1_000);
            create_test_agent_budget(&state, &agent.agent_id, 2_000);
            let spend = spend(
                json!({
                    "operation_key": "advisory-target-job",
                    "account_id": agent.account_id,
                    "amount_cents": 1_500,
                    "reason": "spend above advisory target",
                    "merchant": "Acme Cafe",
                })
                .to_string(),
                &state,
            )
            .expect("spending target should not block spend");
            assert_eq!(spend.decision, "allow");
            assert_eq!(
                spend
                    .budget_hold
                    .expect("spend should settle its agent budget")
                    .consumed_amount_cents,
                1_500
            );
        }

        let restarted =
            ServerState::new_with_db_path(&path).expect("server state should reload from storage");
        let targets =
            list_spending_targets(&restarted, true).expect("spending targets should list");
        assert_eq!(targets.targets.len(), 1);
        assert_eq!(targets.targets[0].status, "active");
        assert_eq!(targets.targets[0].allocated_amount_cents, 2_000);
        assert_eq!(targets.targets[0].exceeded_by_cents, 1_000);
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
                "daily_limit_cents": 5_000,
            })
            .to_string(),
            &state,
        )
        .expect("policy should be added under initialized user");

        create_test_agent_budget(&state, &agent.agent_id, 10_000);

        let request = json!({
            "operation_key": "account-spend-job-1",
            "account_id": agent.account_id,
            "amount_cents": 2_500,
            "reason": "account anchored purchase",
            "merchant": "Acme Cafe",
        });
        let spend_response = spend(request.to_string(), &state)
            .expect("spend should be approved and paid from account");

        assert_eq!(spend_response.account_id, agent.account_id);
        assert_eq!(spend_response.agent_id, agent.agent_id);
        assert_eq!(spend_response.operation_key, "account-spend-job-1");
        let payment = spend_response.payment.expect("allowed spend should pay");
        assert_eq!(payment.account_id, agent.account_id);
        assert_eq!(payment.owner_user_id, user.user_id);
        let retry = spend(request.to_string(), &state)
            .expect("same direct spend operation should return its prior result");
        assert_eq!(retry.operation_key, "account-spend-job-1");
        assert_eq!(
            retry
                .payment
                .expect("retry should return payment")
                .payment_id,
            payment.payment_id
        );
        assert_eq!(
            retry
                .budget_hold
                .expect("retry should return settled hold")
                .consumed_amount_cents,
            2_500
        );
        assert_eq!(
            list_ledger(&state)
                .expect("ledger should list")
                .transactions
                .len(),
            1
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn authorize_spend_freezes_budget_without_payment_or_ledger() {
        let path =
            std::env::temp_dir().join(format!("hubu-api-authorize-spend-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        let _user = init(
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

        create_test_agent_budget(&state, &agent.agent_id, 500);

        let authorization = authorize_spend(
            json!({
                "operation_key": "logo-design-job-1",
                "account_id": agent.account_id,
                "amount_cents": 500,
                "reason": "Generate Project Hubu logo",
                "merchant": "hubu-model-proxy",
            })
            .to_string(),
            &state,
        )
        .expect("spend should authorize");

        assert_eq!(authorization.decision, "allow");
        assert_eq!(authorization.operation_key, "logo-design-job-1");
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
    fn spend_executor_guidance_defines_external_work_boundary() {
        let path = std::env::temp_dir().join(format!(
            "hubu-api-executor-guidance-{}.sqlite",
            UserId::new()
        ));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");

        for path in [
            "/spend/executor/guidance",
            "/.well-known/hubu-spend-executor.json",
        ] {
            let response = route(public_request("GET", path), &state);

            assert_eq!(response.status, 200);
            assert_eq!(response.body["protocol_version"], "hubu-spend-executor-v4");
            assert!(response.body["role_boundary"]["hubu"]
                .as_array()
                .expect("hubu role list should be an array")
                .iter()
                .any(|item| item == "exclusively claim executor spend authorization"));
            assert!(response.body["role_boundary"]["executor"]
                .as_array()
                .expect("executor role list should be an array")
                .iter()
                .any(|item| item == "hold vendor credentials outside Hubu"));
            assert!(response.body["claim_request"]["required"]
                .as_array()
                .expect("required fields should be an array")
                .iter()
                .any(|item| item == "account_id"));
            assert!(response.body["claim_request"]["required"]
                .as_array()
                .expect("required fields should be an array")
                .iter()
                .any(|item| item == "operation_key"));
            assert!(response.body["authorization_request"]["required"]
                .as_array()
                .expect("authorization required fields should be an array")
                .iter()
                .any(|item| item == "operation_key"));
            assert_eq!(
                response.body["release_request"]["required"],
                json!(["agent_id", "operation_key"])
            );
            assert!(response.body["settle_request"]["required"]
                .as_array()
                .expect("settlement fields should be an array")
                .iter()
                .any(|item| item == "receipt.actual_vendor_cost_cents"));
            assert_eq!(
                response.body["operation_key_policy"]["namespace"],
                json!(["agent_id", "operation_key"])
            );
            assert_eq!(
                response.body["timing"]["profiles"]["default"]["claim_ttl_seconds"],
                900
            );
            assert!(response.body["scope_rules"]
                .as_array()
                .expect("scope rules should be an array")
                .iter()
                .any(|item| item == "account_id, amount_cents, merchant, and task_id must match the original authorized spend"));
            assert_eq!(
                response.body["routes"]["reconciliation_queue"],
                "GET /spend/executor/reconciliation"
            );
            assert_eq!(
                response.body["routes"]["reconcile_vendor_billed"],
                "POST /spend/executor/settle"
            );
            assert!(response.body["reconciliation_request"]["required"]
                .as_array()
                .expect("reconciliation fields should be an array")
                .iter()
                .any(|item| item == "evidence"));
        }

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn executor_can_claim_and_settle_authorized_spend() {
        let (path, state, agent, authorization) = setup_executor_authorization("executor-settle");
        let operation_key = authorization.operation_key.clone();
        let token = authorization
            .auth_token_id
            .clone()
            .expect("authorization should issue a token");
        let authorization_retry = authorize_spend(
            json!({
                "operation_key": operation_key,
                "account_id": agent.account_id,
                "amount_cents": 500,
                "reason": "hubu-logo-demo",
                "merchant": "gongbu.image",
            })
            .to_string(),
            &state,
        )
        .expect("authorization retry should return the original workflow");
        assert_eq!(authorization_retry.decision_id, authorization.decision_id);
        assert_eq!(authorization_retry.operation_key, operation_key);
        assert_eq!(
            authorization_retry.auth_token_id,
            authorization.auth_token_id
        );
        assert_eq!(
            authorization_retry
                .budget_hold
                .as_ref()
                .expect("authorization retry should return the hold")
                .hold_id,
            authorization
                .budget_hold
                .as_ref()
                .expect("authorization should return the hold")
                .hold_id
        );

        let conflict = authorize_spend(
            json!({
                "operation_key": operation_key,
                "account_id": agent.account_id,
                "amount_cents": 499,
                "reason": "hubu-logo-demo",
                "merchant": "gongbu.image",
            })
            .to_string(),
            &state,
        )
        .expect_err("an operation key cannot be reused for different spend scope");
        assert!(conflict.to_string().contains("different spend scope"));

        let request = json!({
            "operation_key": operation_key,
            "spend_auth_token_id": token,
            "account_id": agent.account_id,
            "amount_cents": 500,
            "merchant": "gongbu.image",
            "task_id": "hubu-logo-demo",
        });

        let claim =
            claim_executor_spend(request.to_string(), &state).expect("executor spend should claim");
        assert_eq!(claim.spend.agent_id, agent.agent_id);
        assert_eq!(claim.spend.account_id, agent.account_id);
        assert_eq!(claim.spend.merchant.as_deref(), Some("gongbu.image"));
        assert_eq!(claim.spend.task_id.as_deref(), Some("hubu-logo-demo"));
        assert_eq!(claim.status, "claimed");
        assert_eq!(claim.spend.budget_hold.status, "claimed");

        let retry = claim_executor_spend(request.to_string(), &state)
            .expect("same executor execution should recover its existing claim");
        assert_eq!(retry.claim_id, claim.claim_id);
        assert_eq!(retry.status, "claimed");
        assert_eq!(retry.spend.budget_hold.status, "claimed");

        let finalize = json!({
            "agent_id": agent.agent_id,
            "operation_key": claim.operation_key,
            "receipt": settlement_receipt_json(400),
        });
        let settlement = settle_executor_spend(finalize.to_string(), &state)
            .expect("executor spend should settle");
        assert_eq!(settlement.status, "settled");
        assert_eq!(settlement.spend.budget_hold.status, "settled");
        assert_eq!(settlement.spend.budget_hold.consumed_amount_cents, 400);
        assert_eq!(settlement.spend.budget_hold.frozen_amount_cents, 0);
        assert_eq!(settlement.spend.budget_hold.remaining_amount_cents, 100);
        assert_eq!(settlement.receipt.authorized_max_cents, 500);
        assert_eq!(settlement.receipt.actual_vendor_cost_cents, 400);
        assert_eq!(settlement.receipt.released_amount_cents, 100);

        let replay = settle_executor_spend(finalize.to_string(), &state)
            .expect("identical executor settlement should replay");
        assert_eq!(replay.settlement_id, settlement.settlement_id);
        assert_eq!(replay.status, "settled");
        assert_eq!(replay.spend.budget_hold.consumed_amount_cents, 400);
        assert_eq!(replay.spend.budget_hold.frozen_amount_cents, 0);

        let claim_replay = claim_executor_spend(request.to_string(), &state)
            .expect("claim retry should return the stored terminal workflow state");
        assert_eq!(claim_replay.claim_id, claim.claim_id);
        assert_eq!(claim_replay.status, "settled");

        let authorization_replay = authorize_spend(
            json!({
                "operation_key": operation_key,
                "account_id": agent.account_id,
                "amount_cents": 500,
                "reason": "hubu-logo-demo",
                "merchant": "gongbu.image",
            })
            .to_string(),
            &state,
        )
        .expect("authorization retry should return terminal workflow state");
        assert_eq!(authorization_replay.decision_id, authorization.decision_id);
        assert_eq!(
            authorization_replay.auth_token_id,
            authorization.auth_token_id
        );
        assert_eq!(
            authorization_replay
                .budget_hold
                .expect("terminal authorization replay should return the hold")
                .status,
            "settled"
        );

        let validate_request = json!({
            "spend_auth_token_id": token,
            "account_id": agent.account_id,
            "amount_cents": 500,
            "merchant": "gongbu.image",
            "task_id": "hubu-logo-demo",
        });
        let retry_error = validate_executor_spend(validate_request.to_string(), &state)
            .expect_err("settled token should not validate again");
        assert!(retry_error
            .to_string()
            .contains("spend auth token has already been used"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn agents_owned_by_one_user_can_reuse_operation_keys_independently() {
        let path = std::env::temp_dir().join(format!(
            "hubu-api-agent-job-namespace-{}.sqlite",
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
        .expect("init should create one user");

        let first_agent = register_agent(
            json!({"name": "first-agent", "version": "v1"}).to_string(),
            &state,
        )
        .expect("first agent should register");
        let second_agent = register_agent(
            json!({"name": "second-agent", "version": "v1"}).to_string(),
            &state,
        )
        .expect("second agent should register");

        for agent in [&first_agent, &second_agent] {
            add_policy(
                json!({
                    "agent_id": agent.agent_id,
                    "daily_limit_cents": 500,
                })
                .to_string(),
                &state,
            )
            .expect("agent policy should be added");
            create_test_agent_budget(&state, &agent.agent_id, 500);
        }

        let operation_key = "shared-platform-operation";
        let first_authorization = authorize_spend(
            json!({
                "operation_key": operation_key,
                "account_id": first_agent.account_id,
                "amount_cents": 500,
                "reason": "first-agent-task",
                "merchant": "gongbu.image",
            })
            .to_string(),
            &state,
        )
        .expect("first agent should authorize its job");
        let second_authorization = authorize_spend(
            json!({
                "operation_key": operation_key,
                "account_id": second_agent.account_id,
                "amount_cents": 500,
                "reason": "second-agent-task",
                "merchant": "gongbu.image",
            })
            .to_string(),
            &state,
        )
        .expect("second agent should reuse the same platform operation key");
        assert_ne!(
            first_authorization.decision_id,
            second_authorization.decision_id
        );
        let first_claim_request = json!({
            "operation_key": first_authorization.operation_key,
            "spend_auth_token_id": first_authorization.auth_token_id,
            "account_id": first_agent.account_id,
            "amount_cents": 500,
            "merchant": "gongbu.image",
            "task_id": "first-agent-task",
        });
        let second_claim_request = json!({
            "operation_key": second_authorization.operation_key,
            "spend_auth_token_id": second_authorization.auth_token_id,
            "account_id": second_agent.account_id,
            "amount_cents": 500,
            "merchant": "gongbu.image",
            "task_id": "second-agent-task",
        });
        let first_claim = claim_executor_spend(first_claim_request.to_string(), &state)
            .expect("first agent job should claim");
        let second_claim = claim_executor_spend(second_claim_request.to_string(), &state)
            .expect("second agent job should claim independently");
        assert_ne!(first_claim.claim_id, second_claim.claim_id);

        settle_executor_spend(
            json!({
                "agent_id": first_agent.agent_id,
                "operation_key": first_authorization.operation_key,
                "receipt": settlement_receipt_json(400),
            })
            .to_string(),
            &state,
        )
        .expect("first agent job should settle");

        let first_replay = claim_executor_spend(first_claim_request.to_string(), &state)
            .expect("first agent should recover its terminal state");
        let second_replay = claim_executor_spend(second_claim_request.to_string(), &state)
            .expect("second agent claim should remain independent");
        assert_eq!(first_replay.status, "settled");
        assert_eq!(second_replay.status, "claimed");

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn executor_release_returns_budget_and_blocks_reuse() {
        let (path, state, agent, authorization) = setup_executor_authorization("executor-release");
        let token = authorization
            .auth_token_id
            .clone()
            .expect("authorization should issue a token");
        let request = json!({
            "operation_key": authorization.operation_key,
            "spend_auth_token_id": token,
            "account_id": agent.account_id,
            "amount_cents": 500,
            "merchant": "gongbu.image",
            "task_id": "hubu-logo-demo",
        });

        let claim =
            claim_executor_spend(request.to_string(), &state).expect("executor spend should claim");
        let release_request = json!({
            "agent_id": agent.agent_id,
            "operation_key": claim.operation_key,
        });
        let release = release_executor_spend(release_request.to_string(), &state)
            .expect("executor spend should release");
        assert_eq!(release.status, "released");
        assert_eq!(release.spend.budget_hold.status, "released");
        assert_eq!(release.spend.budget_hold.consumed_amount_cents, 0);
        assert_eq!(release.spend.budget_hold.frozen_amount_cents, 0);
        assert_eq!(release.spend.budget_hold.remaining_amount_cents, 500);

        let replay = release_executor_spend(release_request.to_string(), &state)
            .expect("identical executor release should replay");
        assert_eq!(replay.status, "released");
        assert_eq!(replay.spend.budget_hold.frozen_amount_cents, 0);
        assert_eq!(replay.spend.budget_hold.remaining_amount_cents, 500);

        let validate_request = json!({
            "spend_auth_token_id": token,
            "account_id": agent.account_id,
            "amount_cents": 500,
            "merchant": "gongbu.image",
            "task_id": "hubu-logo-demo",
        });
        let retry_error = validate_executor_spend(validate_request.to_string(), &state)
            .expect_err("released hold should not validate again");
        assert!(retry_error
            .to_string()
            .contains("spend auth token has been revoked"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn expired_claim_lookup_queue_and_human_reconciliation_restore_budget_usability() {
        let mut timing = SpendTimingConfig::default();
        timing
            .profiles
            .get_mut("default")
            .expect("default timing profile should exist")
            .claim_ttl_seconds = 1;
        let (path, state, agent, authorization) =
            setup_executor_authorization_with_timing("executor-reconciliation", timing);
        let claim = claim_executor_spend(
            json!({
                "spend_auth_token_id": authorization
                    .auth_token_id
                    .expect("authorization should issue a token"),
                "account_id": agent.account_id,
                "amount_cents": 500,
                "merchant": "gongbu.image",
                "task_id": "hubu-logo-demo",
                "operation_key": "executor-reconciliation-operation",
            })
            .to_string(),
            &state,
        )
        .expect("executor should claim authorization");

        let active = get_executor_claim(Some(&claim.claim_id), &state)
            .expect("active claim status should be readable");
        assert!(!active.reconciliation_required);
        let mixed_mode_error = finalize_executor_spend(
            json!({
                "claim_id": claim.claim_id,
                "operation_key": "executor-reconciliation-operation",
                "agent_id": agent.agent_id,
                "provider_reference": "must-not-mix",
                "evidence": "must-not-mix",
            })
            .to_string(),
            &state,
            true,
            None,
        )
        .expect_err("executor and reconciliation request fields must not be mixed");
        assert!(mixed_mode_error
            .to_string()
            .contains("did not match any variant"));
        assert!(list_executor_claims_requiring_reconciliation(&state)
            .unwrap()
            .claims
            .is_empty());

        std::thread::sleep(std::time::Duration::from_millis(1_100));

        let normal_error = finalize_executor_spend(
            json!({
                "operation_key": "executor-reconciliation-operation",
                "agent_id": agent.agent_id,
                "receipt": settlement_receipt_json(400),
            })
            .to_string(),
            &state,
            true,
            None,
        )
        .expect_err("normal settlement should reject an expired claim");
        assert!(normal_error
            .to_string()
            .contains("expired and requires reconciliation"));

        let queue = list_executor_claims_requiring_reconciliation(&state)
            .expect("reconciliation queue should list");
        assert_eq!(queue.claims.len(), 1);
        assert_eq!(queue.claims[0].claim_id, claim.claim_id);
        assert!(queue.claims[0].reconciliation_required);
        assert_eq!(queue.claims[0].spend.budget_hold.frozen_amount_cents, 500);

        let reconciliation_body = json!({
            "claim_id": claim.claim_id,
            "provider_reference": "openai-request-abc123",
            "evidence": "Provider usage export shows a completed billed request.",
            "receipt": settlement_receipt_json(400),
        });
        let missing_capability = route(
            authenticated_json_request("/spend/executor/settle", reconciliation_body.clone()),
            &state,
        );
        assert_eq!(missing_capability.status, 400);
        assert!(missing_capability.body["error"]
            .as_str()
            .is_some_and(|error| error.contains("missing human reconciliation capability")));

        let mut executor_capability_request =
            authenticated_json_request("/spend/executor/settle", reconciliation_body.clone());
        executor_capability_request.headers.insert(
            RECONCILIATION_CAPABILITY_HEADER.to_string(),
            TEST_AUTH_TOKEN.to_string(),
        );
        let executor_capability = route(executor_capability_request, &state);
        assert_eq!(executor_capability.status, 400);
        assert!(executor_capability.body["error"]
            .as_str()
            .is_some_and(|error| error.contains("invalid human reconciliation capability")));

        let mut human_request =
            authenticated_json_request("/spend/executor/settle", reconciliation_body);
        human_request.headers.insert(
            RECONCILIATION_CAPABILITY_HEADER.to_string(),
            TEST_RECONCILIATION_TOKEN.to_string(),
        );
        let human_response = route(human_request, &state);
        assert_eq!(human_response.status, 200);
        let reconciled = human_response.body;
        assert_eq!(reconciled["status"], "settled");
        assert_eq!(
            reconciled["reconciliation_outcome"].as_str(),
            Some("vendor_billed")
        );
        assert_eq!(
            reconciled["provider_reference"].as_str(),
            Some("openai-request-abc123")
        );
        assert!(reconciled["reconciled_by_user_id"].is_string());
        assert_eq!(reconciled["spend"]["budget_hold"]["frozen_amount_cents"], 0);
        assert_eq!(
            reconciled["spend"]["budget_hold"]["consumed_amount_cents"],
            400
        );
        assert_eq!(
            reconciled["spend"]["budget_hold"]["remaining_amount_cents"],
            100
        );
        assert!(list_executor_claims_requiring_reconciliation(&state)
            .unwrap()
            .claims
            .is_empty());

        let status = get_executor_claim(Some(&claim.claim_id), &state)
            .expect("reconciled claim status should remain readable");
        assert_eq!(
            status.reconciliation_outcome.as_deref(),
            reconciled["reconciliation_outcome"].as_str()
        );
        assert_eq!(status.evidence.as_deref(), reconciled["evidence"].as_str());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn executor_claim_survives_restart_and_can_settle() {
        let (path, state, agent, authorization) =
            setup_executor_authorization("executor-claim-restart");
        let claim = claim_executor_spend(
            json!({
                "operation_key": authorization.operation_key,
                "spend_auth_token_id": authorization
                    .auth_token_id
                    .expect("authorization should issue a token"),
                "account_id": agent.account_id,
                "amount_cents": 500,
                "merchant": "gongbu.image",
                "task_id": "hubu-logo-demo",
            })
            .to_string(),
            &state,
        )
        .expect("executor should claim before restart");
        drop(state);

        let restarted =
            ServerState::new_with_db_path(&path).expect("server should reload claimed state");
        let authorization_replay = authorize_spend(
            json!({
                "operation_key": authorization.operation_key,
                "account_id": agent.account_id,
                "amount_cents": 500,
                "reason": "hubu-logo-demo",
                "merchant": "gongbu.image",
            })
            .to_string(),
            &restarted,
        )
        .expect("authorization retry should survive restart");
        assert_eq!(authorization_replay.decision_id, authorization.decision_id);
        assert_eq!(
            authorization_replay.operation_key,
            authorization.operation_key
        );
        assert_eq!(
            authorization_replay
                .budget_hold
                .expect("replayed authorization should return the claimed hold")
                .status,
            "claimed"
        );
        let settlement = settle_executor_spend(
            json!({
                "agent_id": agent.agent_id,
                "operation_key": claim.operation_key,
                "receipt": settlement_receipt_json(400),
            })
            .to_string(),
            &restarted,
        )
        .expect("reloaded claim should settle");
        assert_eq!(settlement.status, "settled");
        assert_eq!(settlement.spend.budget_hold.status, "settled");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn authorization_replay_preserves_omitted_workload_profile_after_default_changes() {
        let (path, state, agent, authorization) =
            setup_executor_authorization("authorization-profile-restart");
        drop(state);

        let mut restarted =
            ServerState::new_with_db_path(&path).expect("server should reload authorized spend");
        restarted.spend_timing.default_profile = "new-default".to_string();

        let replay = authorize_spend(
            json!({
                "operation_key": authorization.operation_key,
                "account_id": agent.account_id,
                "amount_cents": 500,
                "reason": "hubu-logo-demo",
                "merchant": "gongbu.image",
            })
            .to_string(),
            &restarted,
        )
        .expect("omitted profile should replay with the stored profile");
        assert_eq!(replay.decision_id, authorization.decision_id);
        assert_eq!(replay.auth_token_id, authorization.auth_token_id);
        assert_eq!(replay.workload_profile, authorization.workload_profile);

        let conflict = authorize_spend(
            json!({
                "operation_key": authorization.operation_key,
                "account_id": agent.account_id,
                "amount_cents": 500,
                "reason": "hubu-logo-demo",
                "merchant": "gongbu.image",
                "workload_profile": "new-default",
            })
            .to_string(),
            &restarted,
        )
        .expect_err("an explicitly changed profile should still conflict");
        assert!(conflict.to_string().contains("different spend scope"));

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn spend_rejects_agent_id_anchor() {
        let path =
            std::env::temp_dir().join(format!("hubu-api-agent-anchor-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        let _user = init(
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

        let missing_operation_key_error = spend(
            json!({
                "account_id": agent.account_id,
                "amount_cents": 2_500,
                "reason": "missing operation key",
            })
            .to_string(),
            &state,
        )
        .expect_err("spend should require a platform operation key");
        assert!(missing_operation_key_error
            .to_string()
            .contains("spend operation_key is required"));

        let client_job_id_error = spend(
            json!({
                "job_id": "client-generated-job-id",
                "account_id": agent.account_id,
                "amount_cents": 2_500,
                "reason": "client supplied canonical id",
            })
            .to_string(),
            &state,
        )
        .expect_err("authorization must reject a client-supplied job id");
        assert!(client_job_id_error
            .to_string()
            .contains("unknown field `job_id`"));

        let error = spend(
            json!({
                "operation_key": "agent-anchor-rejection-job",
                "agent_id": agent.agent_id,
                "amount_cents": 2_500,
                "reason": "agent anchored purchase",
            })
            .to_string(),
            &state,
        )
        .expect_err("spend should reject agent id anchor");

        assert!(error.to_string().contains(
            "spend request must include account_id; agent_id is no longer accepted for spend"
        ));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn yaml_policy_add_and_agent_list_use_registered_user() {
        let path = std::env::temp_dir().join(format!("hubu-api-policy-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
        let user = init(
            json!({
                "username": "alice-example",
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
        assert_eq!(agents.agents[0].owner_user_id, user.user_id);
        assert_eq!(
            agents.agents[0].owner_username.as_deref(),
            Some("alice-example")
        );

        let policy = add_policy(
            json!({
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
        assert_eq!(policy.scope, "user_default");
        assert_eq!(policy.agent_id, None);
        assert_eq!(policy.policy_id, "yaml_demo_policy");
        assert_eq!(policy.policy_version, "demo-1");

        let policies_response = route(authenticated_get_request("/policies"), &state);
        assert_eq!(policies_response.status, 200);
        let policies = policies_response.body["policies"]
            .as_array()
            .expect("policies should be an array");
        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0]["scope"], "user_default");
        assert_eq!(policies[0]["agent_id"], Value::Null);
        assert_eq!(policies[0]["policy_id"], "yaml_demo_policy");
        assert_eq!(policies[0]["policy_version"], "demo-1");
        assert_eq!(policies[0]["default_decision"], "needs_approval");
        assert_eq!(policies[0]["rules"], 1);
        assert!(policies[0]["attached_at"].as_str().is_some());
        assert!(policies[0]["updated_at"].as_str().is_some());

        create_test_agent_budget(&state, &agents.agents[0].agent_id, 10_000);

        let spend = spend(
            json!({
                "operation_key": "yaml-policy-job",
                "account_id": agents.agents[0].account_id,
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
    fn yaml_policy_add_rejects_unsupported_cumulative_limit_fields() {
        let path = std::env::temp_dir().join(format!(
            "hubu-api-policy-unsupported-field-{}.sqlite",
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
                "name": "unsupported-policy-field-agent",
                "version": "v1",
            })
            .to_string(),
            &state,
        )
        .expect("agent should register");

        let error = add_policy(
            json!({
                "agent_id": agent.agent_id,
                "policy_yaml": r#"
id: daily_limit_policy
version: demo-1
owner_user_id: 00000000-0000-4000-8000-000000000000
default_effect: needs_approval
daily_limit_cents: 5000
rules: []
"#,
            })
            .to_string(),
            &state,
        )
        .expect_err("unsupported cumulative policy field should fail");

        assert!(error.to_string().contains("failed to parse policy yaml"));
        assert!(error.to_string().contains("daily_limit_cents"));
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

        create_test_agent_budget(&state, &agent.agent_id, 10_000);

        let spend = spend(
            json!({
                "operation_key": "failed-payment-job",
                "account_id": agent.account_id,
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
        let _user = init(
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

        create_test_agent_budget(&state, &agent.agent_id, 1_000);

        let response = route(
            authenticated_json_request(
                "/spend",
                json!({
                    "operation_key": "over-budget-job",
                    "account_id": agent.account_id,
                    "amount_cents": 2_500,
                    "reason": "over budget purchase",
                    "merchant": "Acme Cafe",
                }),
            ),
            &state,
        );

        assert_eq!(response.status, 200);
        assert_eq!(response.body["operation_key"], "over-budget-job");
        assert_eq!(response.body["decision"], "deny");
        assert_eq!(response.body["auth_token_id"], Value::Null);
        assert_eq!(response.body["budget_hold"], Value::Null);
        assert_eq!(response.body["payment"], Value::Null);
        assert!(response.body["reasons"]
            .as_array()
            .expect("reasons should be an array")
            .iter()
            .any(|reason| reason == "budget does not have enough remaining balance"));

        let budgets = list_budgets(&state, false).expect("budgets should list");
        assert_eq!(budgets.budgets[0].remaining_amount_cents, 1_000);
        assert_eq!(budgets.budgets[0].frozen_amount_cents, 0);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn duplicate_agent_registration_remains_blocked_after_restart() {
        let path = std::env::temp_dir().join(format!("hubu-api-restart-{}.sqlite", UserId::new()));
        let _user = {
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
            user
        };

        let restarted =
            ServerState::new_with_db_path(&path).expect("server state should reload from storage");
        let error = register_agent(
            json!({
                "name": "settlement-agent",
                "version": "v1",
            })
            .to_string(),
            &restarted,
        )
        .expect_err("duplicate agent registration should remain blocked after restart");

        assert_eq!(
            error.to_string(),
            "agent is already registered for this owner"
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn governance_and_payment_audit_state_survive_restart() {
        let path = std::env::temp_dir().join(format!(
            "hubu-api-governance-restart-{}.sqlite",
            UserId::new()
        ));
        let (user_id, account_id) = {
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
            create_test_agent_budget(&state, &agent.agent_id, 10_000);
            spend(
                json!({
                    "operation_key": "restart-audit-job",
                    "account_id": agent.account_id,
                    "amount_cents": 2_500,
                    "reason": "restart audit purchase",
                    "merchant": "Acme Cafe",
                })
                .to_string(),
                &state,
            )
            .expect("spend should be approved and paid");
            (user.user_id, agent.account_id)
        };

        let restarted =
            ServerState::new_with_db_path(&path).expect("server state should reload from storage");
        let budgets = list_budgets(&restarted, false).expect("budgets should reload");
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
                "operation_key": "post-restart-policy-job",
                "account_id": account_id,
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

    fn setup_executor_authorization(
        test_name: &str,
    ) -> (
        std::path::PathBuf,
        ServerState,
        RegisterAgentHttpResponse,
        SpendHttpResponse,
    ) {
        setup_executor_authorization_with_timing(test_name, SpendTimingConfig::default())
    }

    fn setup_executor_authorization_with_timing(
        test_name: &str,
        timing: SpendTimingConfig,
    ) -> (
        std::path::PathBuf,
        ServerState,
        RegisterAgentHttpResponse,
        SpendHttpResponse,
    ) {
        let path =
            std::env::temp_dir().join(format!("hubu-api-{test_name}-{}.sqlite", UserId::new()));
        let state = ServerState::new_with_db_path_and_spend_timing(&path, timing)
            .expect("server state should initialize");
        let _user = init(
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
                "name": format!("{test_name}-agent"),
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
        .expect("policy should allow executor spend");

        create_test_agent_budget(&state, &agent.agent_id, 500);

        let authorization = authorize_spend(
            json!({
                "operation_key": format!("{test_name}-operation"),
                "account_id": agent.account_id,
                "amount_cents": 500,
                "reason": "hubu-logo-demo",
                "merchant": "gongbu.image",
            })
            .to_string(),
            &state,
        )
        .expect("spend should authorize");

        (path, state, agent, authorization)
    }
}
