//! Per-agent credentials at the route layer (HUB-357, v0.3.1 spec section 5).
//!
//! A presented bearer resolves to a [`Principal`]: the owner, the shared
//! v0.3.0 agent credential, one agent's own credential, or a registration
//! credential. The secret never reaches Hubu: a registration brings only a
//! credential id and the SHA-256 verification hash of a secret the client
//! minted, and the owner's confirmation activates it.
//!
//! What this module enforces:
//! - A pending agent credential is refused everywhere with
//!   `agent_pending_owner_confirmation`; a revoked one with
//!   `agent_credential_revoked` on its next request.
//! - A registration credential reaches only `POST /agents/register`.
//! - On the spend routes, an agent credential cannot name another account
//!   (`agent_account_mismatch`), and the shared credential no longer works
//!   for an account that has its own credential.
//!
//! Sessions on the spend path, an optional `account_id`, and scoped reads
//! follow in HUB-162 and HUB-360.
use super::*;
use hubu_common::models::AgentIdentity;
use hubu_core::agent_credential::{
    parse_credential, AgentCredential, AgentCredentialCheck, AgentCredentialError,
    AgentCredentialStatus, CredentialKind, NewAgentCredential, RecordedCredential,
    RegistrationCredential,
};

pub(super) const REGISTRATION_CREDENTIALS_ROUTE: &str = "/agents/registration-credentials";
pub(super) const AGENT_CONFIRM_ROUTE: &str = "/agents/confirm";
pub(super) const AGENT_CREDENTIAL_REVOKE_ROUTE: &str = "/agents/credential/revoke";
pub(super) const AGENT_CREDENTIAL_ROTATE_ROUTE: &str = "/agents/credential/rotate";
const MAX_CONFIRM_AGENTS: usize = 50;
const MAX_HARNESS_CHARS: usize = 64;

/// Who presented a request.
#[derive(Clone, Debug)]
pub(super) struct Principal {
    pub(super) class: CredentialClass,
    /// Set when an agent presented its own credential. `None` with
    /// [`CredentialClass::Agent`] means the shared v0.3.0 agent credential.
    pub(super) agent: Option<AgentCredential>,
    pub(super) registration: Option<RegistrationCredential>,
}

impl Principal {
    pub(super) fn shared(class: CredentialClass) -> Self {
        Self {
            class,
            agent: None,
            registration: None,
        }
    }

    pub(super) fn owner() -> Self {
        Self::shared(CredentialClass::Owner)
    }
}

/// A refusal with its own status and error code. Handlers return it through
/// `anyhow`, and the route layer turns it into the response unchanged.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub(super) struct CredentialRefusal {
    pub(super) status: u16,
    pub(super) error_code: &'static str,
    pub(super) message: String,
    pub(super) action: &'static str,
}

impl CredentialRefusal {
    fn new(
        status: u16,
        error_code: &'static str,
        action: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status,
            error_code,
            message: message.into(),
            action,
        }
    }

    pub(super) fn response(&self) -> HttpResponse {
        HttpResponse {
            status: self.status,
            body: json!({
                "error": self.message,
                "error_code": self.error_code,
                "retry_guidance": { "action": self.action, "message": guidance(self.action) },
            }),
        }
    }

    fn pending() -> Self {
        Self::new(
            403,
            "agent_pending_owner_confirmation",
            "wait_for_owner",
            "this agent is waiting for the owner to confirm it; it cannot spend until the owner runs `hubu agent confirm` or confirms it on the Agents page",
        )
    }

    fn account_mismatch(message: impl Into<String>) -> Self {
        Self::new(403, "agent_account_mismatch", "stop", message)
    }
}

fn guidance(action: &str) -> &'static str {
    match action {
        "wait_for_owner" => "do not retry until the owner confirms this agent",
        "ask_owner" => "do not retry; ask the owner to act",
        "register_again" => "register again to submit a new credential for the owner to confirm",
        "fix_request" => "correct the request before retrying",
        _ => "do not retry this request",
    }
}

pub(super) fn credential_refusal_response(error: &anyhow::Error) -> Option<HttpResponse> {
    error
        .downcast_ref::<CredentialRefusal>()
        .map(CredentialRefusal::response)
}

fn refusal_for(error: AgentCredentialError) -> anyhow::Error {
    let refusal = match &error {
        AgentCredentialError::AlreadyRegistered => CredentialRefusal::new(
            409,
            "agent_already_registered",
            "ask_owner",
            error.to_string(),
        ),
        AgentCredentialError::TooManyPending => CredentialRefusal::new(
            429,
            "too_many_pending_agents",
            "ask_owner",
            format!("{error}; ask the owner to confirm or let them expire"),
        ),
        AgentCredentialError::CredentialIdConflict => CredentialRefusal::new(
            409,
            "agent_credential_id_conflict",
            "fix_request",
            "credential id is already in use; mint a new credential",
        ),
        AgentCredentialError::InvalidCredentialId
        | AgentCredentialError::InvalidVerificationHash => CredentialRefusal::new(
            400,
            "invalid_agent_credential",
            "fix_request",
            error.to_string(),
        ),
        AgentCredentialError::NotPending => {
            CredentialRefusal::new(409, "agent_not_pending", "stop", error.to_string())
        }
        AgentCredentialError::NotActive => CredentialRefusal::new(
            409,
            "agent_credential_not_active",
            "stop",
            error.to_string(),
        ),
        AgentCredentialError::NoCredential => {
            CredentialRefusal::new(404, "agent_credential_not_found", "stop", error.to_string())
        }
        AgentCredentialError::Storage(_) => return anyhow::Error::new(error),
    };
    anyhow::Error::new(refusal)
}

fn lock_credentials(
    state: &ServerState,
) -> Result<std::sync::MutexGuard<'_, hubu_core::agent_credential::AgentCredentialManager>> {
    state
        .agent_credentials
        .lock()
        .map_err(|_| anyhow!("agent credential store lock poisoned"))
}

/// Resolves a bearer that is neither the owner nor the shared agent
/// credential. Pending and revoked credentials are refused here, before any
/// route runs.
pub(super) fn resolve_principal(token: &str, state: &ServerState) -> Result<Principal> {
    let invalid = || {
        anyhow::Error::new(CredentialRefusal::new(
            401,
            "agent_credential_invalid",
            "ask_owner",
            "invalid authorization bearer token",
        ))
    };
    let Some(presented) = parse_credential(token) else {
        return Err(anyhow!("invalid authorization bearer token"));
    };
    let now = Utc::now();
    let mut credentials = lock_credentials(state)?;
    match presented.kind {
        CredentialKind::Registration => credentials
            .check_registration(&presented, now)?
            .map(|registration| Principal {
                class: CredentialClass::Registration,
                agent: None,
                registration: Some(registration),
            })
            .ok_or_else(invalid),
        CredentialKind::Agent => match credentials.check_agent(&presented, now)? {
            AgentCredentialCheck::Active(credential) => Ok(Principal {
                class: CredentialClass::Agent,
                agent: Some(credential),
                registration: None,
            }),
            AgentCredentialCheck::Pending(_) => Err(CredentialRefusal::pending().into()),
            AgentCredentialCheck::Revoked => Err(CredentialRefusal::new(
                401,
                "agent_credential_revoked",
                "ask_owner",
                "the owner revoked this agent's credential",
            )
            .into()),
            AgentCredentialCheck::Expired => Err(CredentialRefusal::new(
                401,
                "agent_credential_invalid",
                "register_again",
                format!(
                    "this registration expired after {} days without the owner's confirmation",
                    hubu_core::agent_credential::PENDING_CREDENTIAL_TTL_DAYS
                ),
            )
            .into()),
            AgentCredentialCheck::Invalid => Err(invalid()),
        },
    }
}

/// A registration credential reaches only `POST /agents/register`.
pub(super) fn registration_scope_refusal(
    request: &HttpRequest,
    principal: &Principal,
) -> Option<HttpResponse> {
    (principal.class == CredentialClass::Registration
        && !(request.method == "POST" && request.path == "/agents/register"))
        .then(|| {
            CredentialRefusal::new(
                403,
                "registration_credential_scope",
                "stop",
                format!(
                    "{} {} is not available to a registration credential; it can only submit registrations",
                    request.method, request.path
                ),
            )
            .response()
        })
}

/// Binds the spend routes to the presented agent's account (spec 6.1, the
/// part HUB-357 needs). Runs before the handler, so a refused request
/// reserves and charges nothing.
pub(super) fn spend_account_refusal(
    request: &HttpRequest,
    principal: &Principal,
    state: &ServerState,
) -> Option<HttpResponse> {
    if principal.class != CredentialClass::Agent
        || request.method != "POST"
        || !matches!(
            request.path.as_str(),
            "/spend/authorize" | "/spend" | "/spend/void"
        )
    {
        return None;
    }
    let account_pub_id = serde_json::from_str::<Value>(&request.body)
        .ok()?
        .get("account_id")?
        .as_str()?
        .to_string();
    let result = (|| -> Result<Option<CredentialRefusal>> {
        let account = state
            .registration
            .lock()
            .map_err(|_| anyhow!("registration manager lock poisoned"))?
            .account_for_pub_id(&account_pub_id)?;
        match &principal.agent {
            Some(own) => {
                let matches = account
                    .as_ref()
                    .is_some_and(|account| account.id == own.account_id);
                Ok((!matches).then(|| {
                    CredentialRefusal::account_mismatch(
                        "this agent credential belongs to a different account; an agent can spend only from its own account",
                    )
                }))
            }
            // The shared v0.3.0 credential keeps working only for agents that
            // have no credential of their own.
            None => {
                let Some(account) = account else {
                    return Ok(None);
                };
                let credential = lock_credentials(state)?.latest_for_agent(&account.agent_id)?;
                Ok(credential.map(|credential| match credential.status {
                    AgentCredentialStatus::Pending => CredentialRefusal::pending(),
                    _ => CredentialRefusal::account_mismatch(
                        "this account has its own agent credential; the shared agent credential cannot spend from it",
                    ),
                }))
            }
        }
    })();
    match result {
        Ok(refusal) => refusal.map(|refusal| refusal.response()),
        Err(error) => Some(HttpResponse {
            status: 500,
            body: json!({ "error": format!("agent credential check failed: {error}") }),
        }),
    }
}

/// The credential a registration brings: `agent_credential` beside the
/// envelope or simple fields.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SubmittedCredential {
    pub(super) credential_id: String,
    pub(super) verification_hash: String,
    #[serde(default)]
    pub(super) harness: Option<String>,
}

pub(super) fn submitted_credential(body: &str) -> Result<Option<SubmittedCredential>> {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(body) else {
        return Ok(None);
    };
    let Some(value) = map.get("agent_credential") else {
        return Ok(None);
    };
    let credential: SubmittedCredential = serde_json::from_value(value.clone()).map_err(|error| {
        CredentialRefusal::new(
            400,
            "invalid_agent_credential",
            "fix_request",
            format!(
                "agent_credential accepts only credential_id, verification_hash and harness: {error}"
            ),
        )
    })?;
    if let Some(harness) = &credential.harness {
        validate_harness(harness)?;
    }
    Ok(Some(credential))
}

fn validate_harness(harness: &str) -> Result<()> {
    let valid = !harness.is_empty()
        && harness.chars().count() <= MAX_HARNESS_CHARS
        && harness
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if valid {
        Ok(())
    } else {
        Err(CredentialRefusal::new(
            400,
            "invalid_agent_credential",
            "fix_request",
            format!(
                "harness must be 1 to {MAX_HARNESS_CHARS} ASCII letters, digits, `-`, `_` or `.`"
            ),
        )
        .into())
    }
}

/// Checks a registration against the presenting principal before anything is
/// created, so a refused registration leaves no new agent behind.
pub(super) fn precheck_registration(
    principal: &Principal,
    submitted: Option<&SubmittedCredential>,
    existing_agent: Option<&AgentId>,
    owner_user_id: &UserId,
    state: &ServerState,
) -> Result<()> {
    if principal.class == CredentialClass::Registration && submitted.is_none() {
        return Err(CredentialRefusal::new(
            400,
            "agent_credential_required",
            "fix_request",
            "a registration credential must submit agent_credential with credential_id and verification_hash",
        )
        .into());
    }
    if let Some(registration) = &principal.registration {
        if &registration.owner_user_id != owner_user_id {
            return Err(CredentialRefusal::new(
                403,
                "registration_credential_scope",
                "stop",
                "this registration credential belongs to another owner",
            )
            .into());
        }
    }
    if let Some(own) = &principal.agent {
        // An agent's own credential may re-register only that agent (spec
        // 5.7), and cannot bring a second credential.
        if existing_agent != Some(&own.agent_id) {
            return Err(CredentialRefusal::account_mismatch(
                "an agent credential can register only its own agent",
            )
            .into());
        }
        if submitted.is_some_and(|submitted| submitted.credential_id != own.credential_id) {
            return Err(refusal_for(AgentCredentialError::AlreadyRegistered));
        }
        return Ok(());
    }
    let Some(submitted) = submitted else {
        return Ok(());
    };
    lock_credentials(state)?
        .can_record(
            owner_user_id,
            existing_agent,
            &submitted.credential_id,
            &submitted.verification_hash,
            Utc::now(),
        )
        .map_err(refusal_for)
}

/// Records the submitted credential as pending for the registered agent and
/// returns the agent's credential status for the response.
pub(super) fn record_registration(
    principal: &Principal,
    submitted: Option<SubmittedCredential>,
    agent: &AgentIdentity,
    account: &AgentAccount,
    state: &ServerState,
) -> Result<Option<&'static str>> {
    let now = Utc::now();
    if let Some(own) = &principal.agent {
        return Ok(Some(own.display_status(now)));
    }
    let mut credentials = lock_credentials(state)?;
    let Some(submitted) = submitted else {
        return Ok(credentials
            .live_for_agent(&agent.id)?
            .map(|credential| credential.display_status(now)));
    };
    let harness = submitted.harness.or_else(|| {
        principal
            .registration
            .as_ref()
            .map(|registration| registration.harness.clone())
    });
    let recorded = credentials
        .record_pending(
            NewAgentCredential {
                credential_id: submitted.credential_id,
                verification_hash: submitted.verification_hash,
                owner_user_id: agent.owner_user_id.clone(),
                agent_id: agent.id.clone(),
                account_id: account.id.clone(),
                harness,
            },
            now,
        )
        .map_err(refusal_for)?;
    if let RecordedCredential::Created(credential) = &recorded {
        log_event(
            "info",
            "agent_credential_pending",
            json!({
                "agent_pub_id": agent.pub_id,
                "credential_id": credential.credential_id,
                "harness": credential.harness,
            }),
        );
    }
    Ok(Some(recorded.credential().display_status(now)))
}

/// Credential status for each agent id, for `GET /agents` and the portal.
pub(super) fn credential_statuses(
    agent_ids: impl IntoIterator<Item = AgentId>,
    state: &ServerState,
) -> Result<HashMap<AgentId, &'static str>> {
    let credentials = lock_credentials(state)?;
    let now = Utc::now();
    let mut statuses = HashMap::new();
    for agent_id in agent_ids {
        if let Some(credential) = credentials.latest_for_agent(&agent_id)? {
            statuses.insert(agent_id, credential.display_status(now));
        }
    }
    Ok(statuses)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrationCredentialHttpRequest {
    harness: String,
    credential_id: String,
    verification_hash: String,
}

/// `POST /agents/registration-credentials` (owner bearer). Records the hash
/// of a registration credential that `hubu init <harness>` minted and wrote
/// into that harness's configuration. It grants no spend authority: a
/// registration it submits stays pending until the owner confirms.
pub(super) fn record_registration_credential(body: &str, state: &ServerState) -> Result<Value> {
    let request: RegistrationCredentialHttpRequest = serde_json::from_str(body)?;
    validate_harness(&request.harness)?;
    let user = authenticated_user(state)?;
    let recorded = lock_credentials(state)?
        .record_registration_credential(
            &user.id,
            &request.harness,
            &request.credential_id,
            &request.verification_hash,
            Utc::now(),
        )
        .map_err(refusal_for)?;
    Ok(json!({
        "credential_id": recorded.credential_id,
        "harness": recorded.harness,
        "status": "active",
        "created_at": recorded.created_at.to_rfc3339(),
    }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfirmAgentsHttpRequest {
    agent_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RevokeCredentialHttpRequest {
    agent_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RotateCredentialHttpRequest {
    agent_id: String,
    credential_id: String,
    verification_hash: String,
}

fn parse_confirm(body: &str) -> Result<ConfirmAgentsHttpRequest> {
    let request: ConfirmAgentsHttpRequest = serde_json::from_str(body)
        .map_err(|error| anyhow!("agent confirm accepts only agent_ids: {error}"))?;
    if request.agent_ids.is_empty() || request.agent_ids.len() > MAX_CONFIRM_AGENTS {
        return Err(anyhow!(
            "agent confirm needs 1 to {MAX_CONFIRM_AGENTS} agent_ids"
        ));
    }
    let mut seen = std::collections::HashSet::new();
    if !request.agent_ids.iter().all(|id| seen.insert(id.as_str())) {
        return Err(anyhow!("agent confirm lists an agent more than once"));
    }
    Ok(request)
}

/// Resolves an owned agent by public id, or reports it as unknown.
fn owned_agent(state: &ServerState, owner: &UserId, agent_pub_id: &str) -> Result<AgentIdentity> {
    let registration = state
        .registration
        .lock()
        .map_err(|_| anyhow!("registration manager lock poisoned"))?;
    registration
        .agent_id_for_pub_id(agent_pub_id)?
        .map(|agent_id| registration.agent_for_id(&agent_id))
        .transpose()?
        .flatten()
        .filter(|agent| &agent.owner_user_id == owner)
        .ok_or_else(|| {
            anyhow::Error::new(CredentialRefusal::new(
                404,
                "agent_not_found",
                "stop",
                format!(
                    "unknown agent `{}`",
                    owner_presence::sanitize_id(agent_pub_id)
                ),
            ))
        })
}

/// The owner dialog for the three owner actions (spec 5.5 and 5.6). Returns
/// the action name, the reason line, and the targets.
pub(super) fn owner_action_spec(
    request: &HttpRequest,
    state: &ServerState,
) -> Result<Option<(&'static str, String, Vec<String>)>> {
    let user = authenticated_user(state)?;
    let label = |agent: &AgentIdentity| owner_presence::prompt_label(&agent.display_name);
    match request.path.as_str() {
        AGENT_CONFIRM_ROUTE => {
            let request = parse_confirm(&request.body)?;
            // Resolve agents before taking the credential lock, keeping the
            // registration-then-credentials lock order used by registration.
            let agents = request
                .agent_ids
                .iter()
                .map(|id| owned_agent(state, &user.id, id))
                .collect::<Result<Vec<_>>>()?;
            let credentials = lock_credentials(state)?;
            let now = Utc::now();
            let mut names = Vec::new();
            let mut harnesses = Vec::new();
            for agent in &agents {
                let credential = credentials
                    .live_for_agent(&agent.id)?
                    .filter(|credential| {
                        credential.status == AgentCredentialStatus::Pending
                            && !credential.is_expired(now)
                    })
                    .ok_or_else(|| refusal_for(AgentCredentialError::NotPending))?;
                names.push(format!("\"{}\"", label(agent)));
                if let Some(harness) = credential.harness {
                    harnesses.push(owner_presence::prompt_label(&harness));
                }
            }
            harnesses.sort();
            harnesses.dedup();
            let on = if harnesses.is_empty() {
                String::new()
            } else {
                format!(" on {} (reported by the agent)", harnesses.join(", "))
            };
            let reason = match names.as_slice() {
                [one] => format!("confirm new agent {one}{on}"),
                many => format!("confirm {} agents{on}: {}", many.len(), many.join(", ")),
            };
            Ok(Some((
                "agent.confirm",
                reason,
                request
                    .agent_ids
                    .iter()
                    .map(|id| owner_presence::sanitize_id(id))
                    .collect(),
            )))
        }
        AGENT_CREDENTIAL_REVOKE_ROUTE => {
            let request: RevokeCredentialHttpRequest = serde_json::from_str(&request.body)
                .map_err(|error| anyhow!("credential revoke accepts only agent_id: {error}"))?;
            let agent = owned_agent(state, &user.id, &request.agent_id)?;
            Ok(Some((
                "agent.credential.revoke",
                format!("revoke the credential of agent \"{}\"", label(&agent)),
                vec![owner_presence::sanitize_id(&request.agent_id)],
            )))
        }
        AGENT_CREDENTIAL_ROTATE_ROUTE => {
            let request: RotateCredentialHttpRequest = serde_json::from_str(&request.body)
                .map_err(|error| {
                    anyhow!(
                        "credential rotate accepts only agent_id, credential_id and verification_hash: {error}"
                    )
                })?;
            let agent = owned_agent(state, &user.id, &request.agent_id)?;
            Ok(Some((
                "agent.credential.rotate",
                format!("replace the credential of agent \"{}\"", label(&agent)),
                vec![owner_presence::sanitize_id(&request.agent_id)],
            )))
        }
        _ => Ok(None),
    }
}

fn credential_http(credential: &AgentCredential, agent_pub_id: &str) -> Value {
    let now = Utc::now();
    json!({
        "agent_id": agent_pub_id,
        "credential_id": credential.credential_id,
        "credential_status": credential.display_status(now),
        "harness": credential.harness,
        "activated_at": credential.activated_at.map(|at| at.to_rfc3339()),
        "revoked_at": credential.revoked_at.map(|at| at.to_rfc3339()),
    })
}

/// `POST /agents/confirm` (owner action). Activates every listed pending
/// agent, all or none.
pub(super) fn confirm_agents(body: &str, state: &ServerState) -> Result<Value> {
    let request = parse_confirm(body)?;
    let user = authenticated_user(state)?;
    let agents = request
        .agent_ids
        .iter()
        .map(|id| owned_agent(state, &user.id, id))
        .collect::<Result<Vec<_>>>()?;
    let ids: Vec<AgentId> = agents.iter().map(|agent| agent.id.clone()).collect();
    let confirmed = lock_credentials(state)?
        .confirm(&user.id, &ids, Utc::now())
        .map_err(refusal_for)?;
    log_event(
        "info",
        "agent_credentials_confirmed",
        json!({ "agent_pub_ids": request.agent_ids }),
    );
    Ok(json!({
        "agents": confirmed
            .iter()
            .zip(&agents)
            .map(|(credential, agent)| credential_http(credential, &agent.pub_id))
            .collect::<Vec<_>>(),
    }))
}

/// `POST /agents/credential/revoke` (owner action). The credential is refused
/// on its next request; budget, ledger and ids stay.
pub(super) fn revoke_credential(body: &str, state: &ServerState) -> Result<Value> {
    let request: RevokeCredentialHttpRequest = serde_json::from_str(body)?;
    let user = authenticated_user(state)?;
    let agent = owned_agent(state, &user.id, &request.agent_id)?;
    let revoked = lock_credentials(state)?
        .revoke(&user.id, &agent.id, Utc::now())
        .map_err(refusal_for)?;
    log_event(
        "info",
        "agent_credential_revoked",
        json!({ "agent_pub_id": agent.pub_id, "credential_id": revoked.credential_id }),
    );
    Ok(credential_http(&revoked, &agent.pub_id))
}

/// `POST /agents/credential/rotate` (owner action). The owner CLI minted the
/// new secret, wrote it to the agent's credential file, and sends its hash.
pub(super) fn rotate_credential(body: &str, state: &ServerState) -> Result<Value> {
    let request: RotateCredentialHttpRequest = serde_json::from_str(body)?;
    let user = authenticated_user(state)?;
    let agent = owned_agent(state, &user.id, &request.agent_id)?;
    let rotated = lock_credentials(state)?
        .rotate(
            &user.id,
            &agent.id,
            &request.credential_id,
            &request.verification_hash,
            Utc::now(),
        )
        .map_err(refusal_for)?;
    log_event(
        "info",
        "agent_credential_rotated",
        json!({ "agent_pub_id": agent.pub_id, "credential_id": rotated.credential_id }),
    );
    Ok(credential_http(&rotated, &agent.pub_id))
}
