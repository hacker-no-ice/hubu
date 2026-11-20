//! Route-level owner presence enforcement (v0.3.0 spec, sections 4, 5 and 9).
//!
//! Every protected request is classified by credential and by its effective
//! [`RouteAccess`]. Every owner action runs only after `hubu-server` itself
//! raised the presence dialog for that action and the owner confirmed. There
//! is no owner window: one confirmation authorizes exactly the action its
//! dialog named (owner decision, 2026-11-19).
//! Prompts run outside the dispatch lock so a dialog waiting for the owner
//! never blocks other requests.
use super::*;
use hubu_core::persistence::OwnerPresenceRecord;
use owner_presence::{sanitize_id, sanitize_label, OwnerClient, PresenceMethod, PromptRefusal};

const INVALID_OWNER_ACTION_ERROR_CODE: &str = "invalid_owner_action";
const OWNER_ACTION_TARGET_NOT_FOUND_ERROR_CODE: &str = "owner_action_target_not_found";

/// The agent or budget an owner action names does not exist for this owner.
/// Refused before any prompt, so the owner is never asked about it.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct UnknownOwnerTarget(String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActionKind {
    /// Prompts once for this action.
    Action,
    /// A spend approval: prompts once, and Deny closes the request.
    Approval,
}

#[derive(Clone, Debug)]
struct OwnerActionSpec {
    action: &'static str,
    reason: String,
    targets: Vec<String>,
    kind: ActionKind,
}

pub(super) fn route(request: HttpRequest, state: &ServerState) -> HttpResponse {
    if is_public_route(&request) {
        return dispatch_route(request, state);
    }
    let class = match authenticate_request(&request, state) {
        Ok(class) => class,
        Err(error) => {
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
    };
    let access = effective_route_access(&request);
    if !access.admits(class) {
        log_event(
            "warn",
            "http_request_forbidden",
            json!({
                "method": request.method,
                "path": request.path,
                "credential_class": class.as_str(),
                "required": access.as_str(),
                "error_code": OWNER_AUTHORITY_REQUIRED_ERROR_CODE,
            }),
        );
        return owner_authority_required_response(&request);
    }
    let client = match class {
        CredentialClass::Agent => OwnerClient::Agent,
        CredentialClass::Owner => OwnerClient::from_header(
            request
                .headers
                .get(owner_presence::CLIENT_HEADER)
                .map(String::as_str),
        ),
    };
    match (access, request.method.as_str(), request.path.as_str()) {
        (_, "POST", "/spend/approval/prompt") => approval_prompt(request, state, client),
        (RouteAccess::OwnerAction, _, _) => owner_action(request, state, client),
        (RouteAccess::OwnerBearer, "POST", _) => tightening(request, state, client),
        _ => dispatch_route(request, state),
    }
}

fn header<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
    request.headers.get(name).map(String::as_str)
}

fn no_prompt(request: &HttpRequest) -> bool {
    header(request, owner_presence::NO_PROMPT_HEADER)
        .is_some_and(|value| matches!(value.trim(), "1" | "true" | "yes"))
}

fn with_dispatch<T>(state: &ServerState, f: impl FnOnce() -> T) -> T {
    let _dispatch = match state.dispatch.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    f()
}

fn owner_action(request: HttpRequest, state: &ServerState, client: OwnerClient) -> HttpResponse {
    let position = if client == OwnerClient::Cli && request.path == "/spend/approval/resolve" {
        approval_position(&request)
    } else {
        None
    };
    let mut spec = match with_dispatch(state, || owner_action_spec(&request, state)) {
        Ok(Some(spec)) => spec,
        Ok(None) if position.is_some() => return approval_not_pending(),
        // An approval that is no longer pending: the handler replays or
        // rejects it without granting anything new.
        Ok(None) => return dispatch_route(request, state),
        Err(error) => return invalid_owner_action(&request, &error),
    };
    if let Some((position, total)) = position {
        spec.reason.push_str(&format!(" ({position} of {total})"));
    }
    if no_prompt(&request) {
        return presence_required(&spec);
    }
    match (spec.kind, prompt_and_record(state, &spec, client)) {
        (_, Ok(_)) => dispatch_route(request, state),
        // Deny closes the request in every flow (spec 5.6, owner decision
        // 2026-11-19): the owner's own Approve and an agent-raised prompt
        // behave the same.
        (ActionKind::Approval, Err((refusal, response))) => {
            let approval_request_id =
                serde_json::from_str::<ResolveSpendApprovalHttpRequest>(&request.body)
                    .map(|body| body.approval_request_id)
                    .unwrap_or_default();
            with_refusal_outcome(state, &approval_request_id, &refusal, response)
        }
        (ActionKind::Action, Err((_, response))) => response,
    }
}

fn approval_position(request: &HttpRequest) -> Option<(usize, usize)> {
    let (position, total) = header(request, "x-hubu-approval-position")?.split_once('/')?;
    let position = position.parse().ok()?;
    let total = total.parse().ok()?;
    (position > 0 && position <= total).then_some((position, total))
}

/// Tightening writes need no prompt, but each one is recorded so a call made
/// with a stolen owner bearer is visible.
fn tightening(request: HttpRequest, state: &ServerState, client: OwnerClient) -> HttpResponse {
    let action = match request.path.as_str() {
        "/budgets/revoke" => "budget.revoke",
        "/user/spending-target/revoke" => "spending_target.revoke",
        "/spend/approval/resolve" => "spend.approval.deny",
        _ => "owner.tightening",
    };
    let targets = body_targets(
        &request.body,
        &["budget_id", "target_id", "approval_request_id"],
    );
    // Written before the action runs; if it cannot be stored, the action
    // does not run (spec 9).
    let record = OwnerPresenceRecord {
        record_id: new_record_id(),
        recorded_at: Utc::now(),
        action: action.to_string(),
        reason: String::new(),
        outcome: "not_required".to_string(),
        method: None,
        client: client.as_str().to_string(),
        targets,
    };
    if let Err(error) = record_presence(state, &record) {
        return internal_presence_error(&error);
    }
    dispatch_route(request, state)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovalPromptHttpRequest {
    approval_request_id: String,
}

/// `POST /spend/approval/prompt` (spec section 5.6). An `Agent` route: the
/// agent may raise the approval dialog after the owner says yes in chat, but
/// only the owner's confirmation approves.
fn approval_prompt(request: HttpRequest, state: &ServerState, client: OwnerClient) -> HttpResponse {
    let body: ApprovalPromptHttpRequest = match serde_json::from_str(&request.body) {
        Ok(body) => body,
        Err(error) => {
            return HttpResponse {
                status: 400,
                body: json!({
                    "error": format!("approval prompt accepts only approval_request_id: {error}"),
                    "error_code": INVALID_OWNER_ACTION_ERROR_CODE,
                }),
            }
        }
    };
    let spec = match with_dispatch(state, || approval_spec(&body.approval_request_id, state)) {
        Ok(Some(spec)) => spec,
        Ok(None) => return approval_not_pending(),
        Err(error) => return invalid_owner_action(&request, &error),
    };
    if no_prompt(&request) {
        let mut response = presence_required(&spec);
        if let Value::Object(map) = &mut response.body {
            map.insert("approval_status".to_string(), json!("pending"));
        }
        return response;
    }
    match prompt_and_record(state, &spec, client) {
        Ok(_) => {
            let mut response = resolve_approval(state, &body.approval_request_id, "approve");
            // The owner confirmed, but resolution can still fail (for example
            // the budget was revoked while the dialog was open). Report the
            // stored status, not the owner's answer.
            let status = if response.status < 400 {
                "approved".to_string()
            } else {
                with_dispatch(state, || {
                    get_spend_approval(Some(&body.approval_request_id), state)
                        .ok()
                        .map(|approval| approval.status)
                })
                .unwrap_or_else(|| "pending".to_string())
            };
            if let Value::Object(map) = &mut response.body {
                map.insert("approval_status".to_string(), json!(status));
            }
            response
        }
        Err((refusal, response)) => {
            with_refusal_outcome(state, &body.approval_request_id, &refusal, response)
        }
    }
}

fn approval_not_pending() -> HttpResponse {
    HttpResponse {
        status: 409,
        body: json!({
            "error": "the approval request is no longer pending; it was already resolved (approved or denied)",
            "error_code": APPROVAL_NOT_PENDING_ERROR_CODE,
            "retry_guidance": {
                "action": "do_not_retry",
                "message": "a resolved request is closed; if it was denied, make a new spend request if the owner still wants it"
            },
        }),
    }
}

/// Resolves an approval as `decision` under the dispatch lock.
fn resolve_approval(
    state: &ServerState,
    approval_request_id: &str,
    decision: &str,
) -> HttpResponse {
    dispatch_route(
        HttpRequest {
            method: "POST".to_string(),
            path: "/spend/approval/resolve".to_string(),
            query: HashMap::new(),
            headers: HashMap::new(),
            body: json!({
                "approval_request_id": approval_request_id,
                "decision": decision,
            })
            .to_string(),
        },
        state,
    )
}

/// A refused approval prompt: Deny closes the request as denied; a timeout,
/// an unavailable provider, a busy screen or the rate limit leave it pending.
/// The refusal response carries the resulting `approval_status`.
fn with_refusal_outcome(
    state: &ServerState,
    approval_request_id: &str,
    refusal: &PromptRefusal,
    mut response: HttpResponse,
) -> HttpResponse {
    let status = if *refusal == PromptRefusal::Denied {
        let denied = resolve_approval(state, approval_request_id, "deny");
        if denied.status >= 400 {
            log_event(
                "error",
                "approval_prompt_deny_failed",
                json!({ "status": denied.status, "body": denied.body }),
            );
            "pending"
        } else {
            "denied"
        }
    } else {
        "pending"
    };
    if let Value::Object(map) = &mut response.body {
        map.insert("approval_status".to_string(), json!(status));
    }
    response
}

/// Raises the dialog and records the result, confirmed or not. Fails closed
/// when the record cannot be written.
fn prompt_and_record(
    state: &ServerState,
    spec: &OwnerActionSpec,
    client: OwnerClient,
) -> Result<(PresenceMethod, String), (PromptRefusal, HttpResponse)> {
    log_event(
        "info",
        "owner_presence_prompt_started",
        json!({ "action": spec.action, "client": client.as_str() }),
    );
    let result = state.presence.prompt(&spec.reason);
    let record_id = new_record_id();
    let (outcome, method) = match &result {
        Ok(method) => ("confirmed", Some(method.as_str().to_string())),
        Err(refusal) => (refusal.outcome(), None),
    };
    log_event(
        "info",
        "owner_presence_prompt_finished",
        json!({
            "action": spec.action,
            "client": client.as_str(),
            "outcome": outcome,
            "record_id": record_id,
        }),
    );
    let record = OwnerPresenceRecord {
        record_id: record_id.clone(),
        recorded_at: Utc::now(),
        action: spec.action.to_string(),
        reason: spec.reason.clone(),
        outcome: outcome.to_string(),
        method,
        client: client.as_str().to_string(),
        targets: spec.targets.clone(),
    };
    if let Err(error) = record_presence(state, &record) {
        log_event(
            "error",
            "owner_presence_record_failed",
            json!({ "action": spec.action, "error": error.to_string() }),
        );
        let refusal =
            PromptRefusal::Unavailable("the owner presence record could not be stored".into());
        let response = internal_presence_error(&error);
        return Err((refusal, response));
    }
    match result {
        Ok(method) => Ok((method, record_id)),
        Err(refusal) => {
            let response = refusal_response(&refusal, spec);
            Err((refusal, response))
        }
    }
}

fn record_presence(state: &ServerState, record: &OwnerPresenceRecord) -> Result<()> {
    state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?
        .record_owner_presence(record)
        .map_err(Into::into)
}

fn new_record_id() -> String {
    format!(
        "opr_{}",
        owner_presence::random_hex(16).unwrap_or_else(|_| AgentSessionId::new().to_string())
    )
}

fn refusal_response(refusal: &PromptRefusal, spec: &OwnerActionSpec) -> HttpResponse {
    HttpResponse {
        status: refusal.status(),
        body: json!({
            "error": refusal.message(),
            "error_code": refusal.error_code(),
            "owner_action": spec.action,
            "retry_guidance": refusal.retry_guidance(),
        }),
    }
}

fn presence_required(spec: &OwnerActionSpec) -> HttpResponse {
    HttpResponse {
        status: 403,
        body: json!({
            "error": "this owner action needs the owner's confirmation on the Mac",
            "error_code": OWNER_PRESENCE_REQUIRED_ERROR_CODE,
            "owner_action": spec.action,
            "retry_guidance": {
                "action": "ask_owner",
                "message": "retry without X-Hubu-No-Prompt so hubu-server asks the owner to confirm"
            },
        }),
    }
}

fn invalid_owner_action(request: &HttpRequest, error: &anyhow::Error) -> HttpResponse {
    log_event(
        "warn",
        "owner_action_rejected_before_prompt",
        json!({
            "method": request.method,
            "path": request.path,
            "error": error.to_string(),
        }),
    );
    let (status, error_code) = if error.downcast_ref::<UnknownOwnerTarget>().is_some() {
        (404, OWNER_ACTION_TARGET_NOT_FOUND_ERROR_CODE)
    } else {
        (400, INVALID_OWNER_ACTION_ERROR_CODE)
    };
    HttpResponse {
        status,
        body: json!({
            "error": error.to_string(),
            "error_code": error_code,
        }),
    }
}

fn internal_presence_error(error: &anyhow::Error) -> HttpResponse {
    log_event(
        "error",
        "owner_presence_internal_error",
        json!({ "error": error.to_string() }),
    );
    HttpResponse {
        status: 500,
        body: json!({
            "error": "owner presence could not be recorded; the owner action was not performed",
            "error_code": "owner_presence_record_failed",
        }),
    }
}

fn body_targets(body: &str, fields: &[&str]) -> Vec<String> {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    fields
        .iter()
        .filter_map(|field| map.get(*field).and_then(Value::as_str))
        .map(sanitize_id)
        .collect()
}

/// Reads one JSON object field without the handler's full validation.
fn body_object(body: &str) -> Result<serde_json::Map<String, Value>> {
    if body.trim().is_empty() {
        return Ok(serde_json::Map::new());
    }
    match serde_json::from_str::<Value>(body)? {
        Value::Object(map) => Ok(map),
        _ => Err(anyhow!("request body must be a JSON object")),
    }
}

fn body_str<'a>(map: &'a serde_json::Map<String, Value>, field: &str) -> Option<&'a str> {
    map.get(field).and_then(Value::as_str)
}

fn format_money(amount: &str, currency: Option<&str>) -> Result<String> {
    let parsed = amount.trim().parse::<Amount>()?;
    // Accept exactly what the handlers accept (trimmed, any case).
    let currency = normalize_spend_currency(currency)?;
    let money = MoneyHttpResponse::new(parsed, currency);
    Ok(format!(
        "{} {}",
        money.display,
        money.currency.to_uppercase()
    ))
}

fn agent_name(state: &ServerState, agent_pub_id: &str) -> Result<String> {
    let agents = to_json(list_agents_for_scope(state, true)?);
    agents["agents"]
        .as_array()
        .and_then(|agents| {
            agents
                .iter()
                .find(|agent| agent["agent_id"].as_str() == Some(agent_pub_id))
        })
        .and_then(|agent| agent["display_name"].as_str())
        .map(sanitize_label)
        .ok_or_else(|| {
            anyhow::Error::new(UnknownOwnerTarget(format!(
                "unknown agent `{}`",
                sanitize_id(agent_pub_id)
            )))
        })
}

/// Builds the dialog reason for an owner action from stored, validated data
/// (spec section 5.5). `Ok(None)` means the action needs no prompt (an
/// approval that is no longer pending).
fn owner_action_spec(
    request: &HttpRequest,
    state: &ServerState,
) -> Result<Option<OwnerActionSpec>> {
    let action = |action, reason, targets| {
        Ok(Some(OwnerActionSpec {
            action,
            reason,
            targets,
            kind: ActionKind::Action,
        }))
    };
    if request.method == "POST" {
        if let Some(budget_id) = budget_versions_public_id(&request.path) {
            let map = body_object(&request.body)?;
            let amount = body_str(&map, "amount_limit")
                .ok_or_else(|| anyhow!("budget update requires `amount_limit`"))?;
            let budgets = to_json(list_budgets_at(state, true, Utc::now())?);
            let budget = budgets["budgets"]
                .as_array()
                .and_then(|budgets| {
                    budgets
                        .iter()
                        .find(|budget| budget["budget_id"].as_str() == Some(budget_id))
                })
                .ok_or_else(|| {
                    anyhow::Error::new(UnknownOwnerTarget(format!(
                        "unknown budget `{}`",
                        sanitize_id(budget_id)
                    )))
                })?;
            let currency = budget["currency"].as_str();
            let current = budget["amount_limit"]["display"]
                .as_str()
                .map(|display| format!("{display} {}", currency.unwrap_or("usd").to_uppercase()))
                .unwrap_or_else(|| "unknown".to_string());
            let agent_id = budget["agent_id"].as_str().unwrap_or_default();
            let name = agent_name(state, agent_id).unwrap_or_else(|_| sanitize_id(agent_id));
            return action(
                "budget.version.create",
                format!(
                    "set {name}'s budget to {} (now {current})",
                    format_money(amount, currency)?
                ),
                vec![sanitize_id(budget_id), sanitize_id(agent_id)],
            );
        }
    }
    match (request.method.as_str(), request.path.as_str()) {
        ("POST", "/init") => {
            let map = body_object(&request.body)?;
            let username = body_str(&map, "username")
                .map_or_else(|| "the default user".to_string(), sanitize_label);
            action(
                "user.init",
                format!("initialize Hubu for owner {username}"),
                Vec::new(),
            )
        }
        ("POST", "/user/spending-target") => {
            let map = body_object(&request.body)?;
            let amount = body_str(&map, "amount")
                .ok_or_else(|| anyhow!("spending target requires `amount`"))?;
            action(
                "spending_target.set",
                format!("set the spending target to {}", format_money(amount, None)?),
                Vec::new(),
            )
        }
        ("POST", agent_identity::AGENT_RENAME_ROUTE) => {
            let map = body_object(&request.body)?;
            let agent_id = body_str(&map, "agent_id")
                .ok_or_else(|| anyhow!("agent rename requires `agent_id`"))?;
            let new_name =
                body_str(&map, "name").ok_or_else(|| anyhow!("agent rename requires `name`"))?;
            let old_name = agent_name(state, agent_id)?;
            action(
                "agent.rename",
                format!("rename agent {old_name} to {}", sanitize_label(new_name)),
                vec![sanitize_id(agent_id)],
            )
        }
        ("POST", "/policies") => {
            let map = body_object(&request.body)?;
            let agent_id = body_str(&map, "agent_id");
            let scope = match agent_id {
                Some(agent_id) => format!("agent {}", agent_name(state, agent_id)?),
                None => "the default policy".to_string(),
            };
            let reason = match map.get("expected_revision").and_then(Value::as_u64) {
                Some(revision) => format!(
                    "apply policy revision {} to {scope}",
                    revision.saturating_add(1)
                ),
                None => format!("apply a new policy revision to {scope}"),
            };
            action(
                "policy.apply",
                reason,
                agent_id.map(sanitize_id).into_iter().collect(),
            )
        }
        ("POST", "/budgets") => {
            let map = body_object(&request.body)?;
            let amount = body_str(&map, "amount")
                .ok_or_else(|| anyhow!("budget create requires `amount`"))?;
            let agent_id = body_str(&map, "agent_id")
                .ok_or_else(|| anyhow!("budget create requires `agent_id`"))?;
            let name = agent_name(state, agent_id)?;
            action(
                "budget.create",
                format!(
                    "create a {} budget for {name}",
                    format_money(amount, body_str(&map, "currency"))?
                ),
                vec![sanitize_id(agent_id)],
            )
        }
        ("POST", "/spend/approval/resolve") => {
            let request: ResolveSpendApprovalHttpRequest = serde_json::from_str(&request.body)?;
            approval_spec(&request.approval_request_id, state)
        }
        ("POST", "/spend/executor/settle" | "/spend/executor/release") => {
            let map = body_object(&request.body)?;
            let claim_id = body_str(&map, "claim_id")
                .ok_or_else(|| anyhow!("reconciliation requires `claim_id`"))?;
            let billed = if request.path.ends_with("/settle") {
                "billed"
            } else {
                "not billed"
            };
            action(
                "claim.reconcile",
                format!("record claim {} as {billed}", sanitize_id(claim_id)),
                vec![sanitize_id(claim_id)],
            )
        }
        // Every declared owner action has an arm above. Anything else is not
        // a route, and the owner is never asked about it.
        _ => Err(anyhow::Error::new(UnknownOwnerTarget(format!(
            "no route for {} {}",
            request.method, request.path
        )))),
    }
}

/// The approval reason from the stored request, or `None` when it is no
/// longer pending.
fn approval_spec(
    approval_request_id: &str,
    state: &ServerState,
) -> Result<Option<OwnerActionSpec>> {
    let approval = to_json(get_spend_approval(Some(approval_request_id), state)?);
    if approval["status"].as_str() != Some("pending") {
        return Ok(None);
    }
    let review = &approval["review"];
    let amount = format!(
        "{} {}",
        review["amount"]["display"].as_str().unwrap_or("?"),
        review["amount"]["currency"]
            .as_str()
            .unwrap_or("usd")
            .to_uppercase()
    );
    let scope = &review["execution_scope"];
    let provider = scope["provider"]["id"]
        .as_str()
        .filter(|id| *id != "provider:legacy:unresolved")
        .map(|id| id.trim_start_matches("provider:"));
    let model = scope["model"].as_str().or_else(|| {
        scope["capability"]["id"]
            .as_str()
            .map(|id| id.trim_start_matches("capability:"))
    });
    let target = match (provider, model) {
        (Some(provider), Some(model)) => {
            format!("{}/{}", sanitize_id(provider), sanitize_id(model))
        }
        (Some(provider), None) => sanitize_id(provider),
        _ => review["merchant"]
            .as_str()
            .map_or_else(|| "an unspecified target".to_string(), sanitize_id),
    };
    let agent_id = review["agent_id"].as_str().unwrap_or_default();
    let name = agent_name(state, agent_id).unwrap_or_else(|_| sanitize_id(agent_id));
    Ok(Some(OwnerActionSpec {
        action: "spend.approval.approve",
        reason: format!("approve {amount} on {target} for agent {name}"),
        targets: vec![sanitize_id(approval_request_id), sanitize_id(agent_id)],
        kind: ActionKind::Approval,
    }))
}

/// `GET /governance/owner-presence`: newest owner presence records first.
pub(super) fn owner_presence_records(request: &HttpRequest, state: &ServerState) -> Result<Value> {
    let limit = match request.query.get("limit") {
        Some(limit) => limit
            .parse::<usize>()
            .ok()
            .filter(|limit| (1..=200).contains(limit))
            .ok_or_else(|| anyhow!("limit must be between 1 and 200"))?,
        None => 50,
    };
    let records = state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?
        .list_owner_presence_records(limit)?;
    Ok(json!({
        "category": "owner_presence",
        "records": records.into_iter().map(|record| json!({
            "record_id": record.record_id,
            "recorded_at": record.recorded_at.to_rfc3339(),
            "category": "owner_presence",
            "action": record.action,
            "reason": record.reason,
            "outcome": record.outcome,
            "method": record.method,
            "client": record.client,
            "target": record.targets,
        })).collect::<Vec<_>>(),
    }))
}
