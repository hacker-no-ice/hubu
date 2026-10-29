//! HTTP transport for task-scoped budgets: open, close, list and show, plus
//! agent proposals and close requests that a human approves.
//!
//! A human opens and closes directly. An agent proposes or requests close;
//! a human reviews and resolves the request on the task budget approval
//! routes (`/task-budgets/approvals`). Spend approval routes refuse `tba_` ids:
//! a task budget request reserves or returns budget and never authorizes spend.
//!
//! Amounts are exact decimals only; task budgets have no legacy cent fields.
use super::*;
use hubu_core::app::TaskBudgetService;
use hubu_core::budget::{
    public_task_budget_approval_id, public_task_budget_id, CloseTaskBudgetRequest,
    OpenTaskBudgetRequest, RequestTaskBudgetCloseRequest, ResolveTaskBudgetApprovalRequest,
    TaskBudgetApproval, TaskBudgetApprovalDecision, TaskBudgetApprovalKind,
    TaskBudgetApprovalSnapshot, TaskBudgetError, TaskBudgetSnapshot, TaskBudgetState,
    TASK_BUDGET_APPROVAL_ID_PREFIX,
};

pub(super) const OPEN_ROUTE: &str = "/task-budgets";
pub(super) const CLOSE_ROUTE: &str = "/task-budgets/close";
pub(super) const SHOW_ROUTE: &str = "/task-budgets/show";
pub(super) const PROPOSAL_ROUTE: &str = "/task-budgets/proposals";
pub(super) const CLOSE_REQUEST_ROUTE: &str = "/task-budgets/close-requests";
pub(super) const APPROVAL_ROUTE: &str = "/task-budgets/approvals";
pub(super) const APPROVAL_RESOLVE_ROUTE: &str = "/task-budgets/approvals/resolve";

/// A `tba_` id sent to a spend approval route.
#[derive(Debug, thiserror::Error)]
#[error(
    "{approval_request_id} is a task budget approval request, not a spend approval; use {route}"
)]
pub(super) struct NotASpendApproval {
    approval_request_id: String,
    route: &'static str,
}

/// Refuse a task budget approval id on a spend approval route.
pub(super) fn reject_on_spend_route(approval_request_id: &str, route: &'static str) -> Result<()> {
    if is_approval_request_id(approval_request_id) {
        return Err(NotASpendApproval {
            approval_request_id: approval_request_id.to_owned(),
            route,
        }
        .into());
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveTaskBudgetApprovalHttpRequest {
    approval_request_id: String,
    decision: TaskBudgetApprovalHttpDecision,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TaskBudgetApprovalHttpDecision {
    Approve,
    Deny,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenTaskBudgetHttpRequest {
    agent_id: String,
    task_id: String,
    amount: String,
    /// Budget currency (`usd` when omitted).
    currency: Option<String>,
    reason: Option<String>,
    /// Provenance source label, such as `hubu-cli` or `hubu-unified-mcp`.
    source: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CloseTaskBudgetHttpRequest {
    task_budget_id: String,
    source: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestCloseHttpRequest {
    task_budget_id: String,
    /// Why the agent considers the task done, shown to the human.
    reason: Option<String>,
    source: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct TaskBudgetHttpResponse {
    task_budget_id: String,
    agent_id: String,
    budget_id: String,
    task_id: String,
    state: &'static str,
    currency: String,
    amount_limit: MoneyHttpResponse,
    consumed_amount: MoneyHttpResponse,
    frozen_amount: MoneyHttpResponse,
    remaining_amount: MoneyHttpResponse,
    /// Unused allocation returned to the agent budget; set once closed.
    returned_amount: Option<MoneyHttpResponse>,
    outstanding_authorizations: u64,
    settled_authorizations: u64,
    opened_at: String,
    opened_by: String,
    source: String,
    reason: Option<String>,
    closed_at: Option<String>,
    /// The agent's close request waiting on a human, if any.
    close_approval_request_id: Option<String>,
}

/// A task budget approval request on the shared approval link.
#[derive(Debug, Serialize)]
pub(super) struct TaskBudgetApprovalHttpResponse {
    approval_request_id: String,
    /// `task_budget_open` or `task_budget_close`.
    kind: &'static str,
    /// What approving does, in words a human reviewer reads. A task budget
    /// request reserves or returns budget; it never authorizes spend.
    summary: String,
    /// `pending`, `approved`, `denied` or `expired`.
    status: &'static str,
    review: TaskBudgetApprovalReviewHttpResponse,
    resolved_at: Option<String>,
    resolved_by: Option<String>,
    /// The task budget the request opened or would close.
    task_budget: Option<TaskBudgetHttpResponse>,
}

#[derive(Debug, Serialize)]
struct TaskBudgetApprovalReviewHttpResponse {
    agent_id: String,
    task_id: String,
    currency: String,
    amount_limit: MoneyHttpResponse,
    reason: Option<String>,
    requested_at: String,
    requested_by: String,
    source: String,
    expires_at: String,
}

#[derive(Debug, Serialize)]
struct TaskBudgetRequestHttpResponse {
    /// The pending request; absent when an identical task budget is already
    /// open (proposal) or the task budget is already closed (close request).
    approval: Option<TaskBudgetApprovalHttpResponse>,
    task_budget: Option<TaskBudgetHttpResponse>,
    idempotent_replay: bool,
}

#[derive(Debug, Serialize)]
struct TaskBudgetCommandHttpResponse {
    task_budget: TaskBudgetHttpResponse,
    idempotent_replay: bool,
}

#[derive(Debug, Serialize)]
struct ListTaskBudgetsHttpResponse {
    task_budgets: Vec<TaskBudgetHttpResponse>,
    /// Proposals and close requests waiting on a human, oldest first.
    pending_approvals: Vec<TaskBudgetApprovalHttpResponse>,
}

#[derive(Debug, Serialize)]
struct ShowTaskBudgetHttpResponse {
    task_budget: Option<TaskBudgetHttpResponse>,
}

fn provenance_source(source: Option<String>) -> Result<String> {
    let source = source.unwrap_or_else(|| "hubu-api".to_string());
    let trimmed = source.trim();
    if trimmed.is_empty() || trimmed.len() > 64 || trimmed.chars().any(char::is_control) {
        return Err(anyhow!("source must be 1 to 64 printable characters"));
    }
    Ok(trimmed.to_string())
}

pub(super) fn open(body: String, state: &ServerState, now: DateTime<Utc>) -> Result<Value> {
    let request: OpenTaskBudgetHttpRequest = serde_json::from_str(&body)?;
    let amount_limit = request.amount.parse::<Amount>()?;
    let currency = normalize_spend_currency(request.currency.as_deref())?;
    let user = authenticated_user(state)?;
    let user_context = UserContext::new(user.id.clone());
    let agent_id = resolve_agent_id_for_user(&request.agent_id, &user_context, state)?;
    let source = provenance_source(request.source)?;
    // Expired holds return their amount before the allocation check.
    reconcile_expired_budget_holds_at(state, now)?;
    let budgets = state
        .budgets
        .lock()
        .map_err(|_| anyhow!("budget manager lock poisoned"))?;
    let mut governance = state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?;
    let response = TaskBudgetService.open(
        OpenTaskBudgetRequest {
            owner_user_id: user.id.clone(),
            agent_id,
            task_id: request.task_id,
            currency,
            amount_limit,
            actor: user.pub_id.clone(),
            source,
            reason: request.reason,
        },
        &budgets,
        &mut governance,
        now,
    )?;
    drop(governance);
    drop(budgets);
    Ok(to_json(TaskBudgetCommandHttpResponse {
        task_budget: task_budget_response(&response.task_budget, None, state)?,
        idempotent_replay: response.replayed,
    }))
}

pub(super) fn close(body: String, state: &ServerState, now: DateTime<Utc>) -> Result<Value> {
    let request: CloseTaskBudgetHttpRequest = serde_json::from_str(&body)?;
    let user = authenticated_user(state)?;
    let source = provenance_source(request.source)?;
    // An expired hold is not outstanding; sweep it before the close check.
    reconcile_expired_budget_holds_at(state, now)?;
    // Same order as spend reservation: budget manager, then governance.
    let _budgets = state
        .budgets
        .lock()
        .map_err(|_| anyhow!("budget manager lock poisoned"))?;
    let mut governance = state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?;
    let existing = governance
        .load_task_budget(&user.id, request.task_budget_id.trim())?
        .ok_or(TaskBudgetError::UnknownTaskBudget)?;
    let response = TaskBudgetService.close(
        CloseTaskBudgetRequest {
            owner_user_id: user.id.clone(),
            task_budget_id: existing.budget.id,
            actor: user.pub_id.clone(),
            source,
        },
        &mut governance,
        now,
    )?;
    drop(governance);
    Ok(to_json(TaskBudgetCommandHttpResponse {
        task_budget: task_budget_response(&response.task_budget, None, state)?,
        idempotent_replay: response.replayed,
    }))
}

/// `POST /task-budgets/proposals`: an agent proposes a task budget. Nothing
/// is reserved until a human approves it on the approval link.
pub(super) fn propose(body: String, state: &ServerState, now: DateTime<Utc>) -> Result<Value> {
    let request: OpenTaskBudgetHttpRequest = serde_json::from_str(&body)?;
    let amount_limit = request.amount.parse::<Amount>()?;
    let currency = normalize_spend_currency(request.currency.as_deref())?;
    let user = authenticated_user(state)?;
    let user_context = UserContext::new(user.id.clone());
    let agent_id = resolve_agent_id_for_user(&request.agent_id, &user_context, state)?;
    let source = provenance_source(request.source)?;
    reconcile_expired_budget_holds_at(state, now)?;
    let budgets = state
        .budgets
        .lock()
        .map_err(|_| anyhow!("budget manager lock poisoned"))?;
    let mut governance = state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?;
    let response = TaskBudgetService.propose(
        OpenTaskBudgetRequest {
            owner_user_id: user.id.clone(),
            agent_id,
            task_id: request.task_id,
            currency,
            amount_limit,
            actor: user.pub_id.clone(),
            source,
            reason: request.reason,
        },
        &budgets,
        &mut governance,
        now,
    )?;
    drop(governance);
    drop(budgets);
    Ok(to_json(TaskBudgetRequestHttpResponse {
        approval: response
            .approval
            .map(|approval| {
                approval_response(
                    &TaskBudgetApprovalSnapshot {
                        approval,
                        task_budget: None,
                    },
                    now,
                    state,
                )
            })
            .transpose()?,
        task_budget: response
            .task_budget
            .as_ref()
            .map(|snapshot| task_budget_response(snapshot, None, state))
            .transpose()?,
        idempotent_replay: response.replayed,
    }))
}

/// `POST /task-budgets/close-requests`: an agent asks a human to close a task
/// budget. New spend for its task is refused while the request is pending.
pub(super) fn request_close(
    body: String,
    state: &ServerState,
    now: DateTime<Utc>,
) -> Result<Value> {
    let request: RequestCloseHttpRequest = serde_json::from_str(&body)?;
    let user = authenticated_user(state)?;
    let source = provenance_source(request.source)?;
    let mut governance = state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?;
    let existing = governance
        .load_task_budget(&user.id, request.task_budget_id.trim())?
        .ok_or(TaskBudgetError::UnknownTaskBudget)?;
    let response = TaskBudgetService.request_close(
        RequestTaskBudgetCloseRequest {
            owner_user_id: user.id.clone(),
            task_budget_id: existing.budget.id,
            actor: user.pub_id.clone(),
            source,
            reason: request.reason,
        },
        &mut governance,
        now,
    )?;
    drop(governance);
    Ok(to_json(TaskBudgetRequestHttpResponse {
        approval: response
            .approval
            .as_ref()
            .map(|approval| {
                approval_response(
                    &TaskBudgetApprovalSnapshot {
                        approval: approval.clone(),
                        task_budget: None,
                    },
                    now,
                    state,
                )
            })
            .transpose()?,
        task_budget: Some(task_budget_response(
            &response.task_budget,
            response.approval.as_ref(),
            state,
        )?),
        idempotent_replay: response.replayed,
    }))
}

/// Whether an approval link id names a task budget approval request.
pub(super) fn is_approval_request_id(approval_request_id: &str) -> bool {
    approval_request_id.starts_with(TASK_BUDGET_APPROVAL_ID_PREFIX)
}

/// `GET /task-budgets/approvals?approval_request_id=tba_..`: the human review.
pub(super) fn get_approval(
    request: &HttpRequest,
    state: &ServerState,
    now: DateTime<Utc>,
) -> Result<Value> {
    let approval_request_id = approval_request_id(request.query.get("approval_request_id"))?;
    let user = authenticated_user(state)?;
    let snapshot = state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?
        .load_task_budget_approval(&user.id, approval_request_id)?
        .ok_or(TaskBudgetError::UnknownApproval)?;
    Ok(to_json(approval_response(&snapshot, now, state)?))
}

/// Only `tba_` ids name task budget approval requests.
fn approval_request_id(approval_request_id: Option<&String>) -> Result<&str> {
    let approval_request_id = approval_request_id
        .map(String::as_str)
        .ok_or_else(|| anyhow!("approval_request_id is required"))?;
    if !is_approval_request_id(approval_request_id) {
        return Err(TaskBudgetError::UnknownApproval.into());
    }
    Ok(approval_request_id)
}

/// `POST /task-budgets/approvals/resolve`, behind the human approval
/// capability. Approving a proposal opens the task budget; approving a close
/// request closes it and returns the unused amount.
pub(super) fn resolve_approval(
    body: String,
    state: &ServerState,
    now: DateTime<Utc>,
) -> Result<Value> {
    let request: ResolveTaskBudgetApprovalHttpRequest = serde_json::from_str(&body)?;
    let approval_request_id = approval_request_id(Some(&request.approval_request_id))?;
    let approve = matches!(request.decision, TaskBudgetApprovalHttpDecision::Approve);
    let user = authenticated_user(state)?;
    // An expired hold is not outstanding; sweep it before a close approval.
    reconcile_expired_budget_holds_at(state, now)?;
    // Same order as spend reservation: budget manager, then governance.
    let budgets = state
        .budgets
        .lock()
        .map_err(|_| anyhow!("budget manager lock poisoned"))?;
    let mut governance = state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?;
    let approval = governance
        .load_task_budget_approval(&user.id, approval_request_id)?
        .ok_or(TaskBudgetError::UnknownApproval)?;
    let response = TaskBudgetService.resolve(
        ResolveTaskBudgetApprovalRequest {
            owner_user_id: user.id.clone(),
            approval_id: approval.approval.id,
            decision: if approve {
                TaskBudgetApprovalDecision::Approve
            } else {
                TaskBudgetApprovalDecision::Deny
            },
            actor: user.pub_id.clone(),
        },
        &budgets,
        &mut governance,
        now,
    )?;
    drop(governance);
    drop(budgets);
    let mut body = to_json(approval_response(&response.approval, now, state)?);
    body["idempotent_replay"] = json!(response.replayed);
    Ok(body)
}

/// `GET /task-budgets[?agent_id=..][&all=true]`: open task budgets, or all.
pub(super) fn list(request: &HttpRequest, state: &ServerState) -> Result<Value> {
    let user = authenticated_user(state)?;
    let agent_id = request
        .query
        .get("agent_id")
        .map(|agent| resolve_agent_id_for_user(agent, &UserContext::new(user.id.clone()), state))
        .transpose()?;
    let now = Utc::now();
    let governance = state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?;
    let snapshots =
        governance.list_task_budgets(&user.id, agent_id.as_ref(), query_flag(request, "all"))?;
    let pending =
        governance.list_pending_task_budget_approvals(&user.id, agent_id.as_ref(), now)?;
    drop(governance);
    let close_request = |snapshot: &TaskBudgetSnapshot| {
        pending
            .iter()
            .map(|pending| &pending.approval)
            .find(|approval| {
                approval.kind == TaskBudgetApprovalKind::Close
                    && approval.task_budget_id.as_ref() == Some(&snapshot.budget.id)
            })
    };
    let task_budgets = snapshots
        .iter()
        .map(|snapshot| task_budget_response(snapshot, close_request(snapshot), state))
        .collect::<Result<_>>()?;
    let pending_approvals = pending
        .iter()
        .map(|approval| approval_response(approval, now, state))
        .collect::<Result<_>>()?;
    Ok(to_json(ListTaskBudgetsHttpResponse {
        task_budgets,
        pending_approvals,
    }))
}

/// `GET /task-budgets/show?task_budget_id=tbg_..` or
/// `?agent_id=..&task_id=..[&currency=..]` for the task's open budget.
pub(super) fn show(request: &HttpRequest, state: &ServerState) -> Result<Value> {
    let user = authenticated_user(state)?;
    let governance = state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?;
    let snapshot = match (
        request.query.get("task_budget_id"),
        request.query.get("agent_id"),
        request.query.get("task_id"),
    ) {
        (Some(public_id), None, None) => governance.load_task_budget(&user.id, public_id)?,
        (None, Some(agent), Some(task_id)) => {
            let agent_id =
                resolve_agent_id_for_user(agent, &UserContext::new(user.id.clone()), state)?;
            let currency =
                normalize_spend_currency(request.query.get("currency").map(String::as_str))?;
            use hubu_core::persistence::TaskBudgetRepository;
            governance
                .open_task_budget_for_spend(&agent_id, task_id, currency)?
                .filter(|snapshot| snapshot.budget.owner_user_id == user.id)
        }
        _ => {
            return Err(anyhow!(
                "show a task budget by task_budget_id, or by agent_id and task_id"
            ))
        }
    };
    let close_request = snapshot
        .as_ref()
        .map(|snapshot| {
            governance.pending_task_budget_close_request(&snapshot.budget.id, Utc::now())
        })
        .transpose()?
        .flatten();
    drop(governance);
    let task_budget = snapshot
        .as_ref()
        .map(|snapshot| task_budget_response(snapshot, close_request.as_ref(), state))
        .transpose()?;
    Ok(to_json(ShowTaskBudgetHttpResponse { task_budget }))
}

pub(super) fn task_budget_response(
    snapshot: &TaskBudgetSnapshot,
    close_request: Option<&TaskBudgetApproval>,
    state: &ServerState,
) -> Result<TaskBudgetHttpResponse> {
    let budget = &snapshot.budget;
    let currency = budget.currency;
    Ok(TaskBudgetHttpResponse {
        task_budget_id: public_task_budget_id(&budget.id),
        agent_id: registration_agent_pub_id(&budget.agent_id, state)?,
        budget_id: public_budget_id(&budget.parent_budget_id),
        task_id: budget.task_id.clone(),
        state: budget.state.as_str(),
        currency: currency.to_string(),
        amount_limit: MoneyHttpResponse::new(budget.amount_limit, currency),
        consumed_amount: MoneyHttpResponse::new(snapshot.usage.consumed_amount, currency),
        frozen_amount: MoneyHttpResponse::new(snapshot.usage.frozen_amount, currency),
        remaining_amount: MoneyHttpResponse::new(snapshot.remaining_amount(), currency),
        returned_amount: (budget.state == TaskBudgetState::Closed)
            .then(|| MoneyHttpResponse::new(snapshot.returned_amount(), currency)),
        outstanding_authorizations: snapshot.usage.outstanding_hold_count,
        settled_authorizations: snapshot.usage.settled_hold_count,
        opened_at: budget.opened_at.to_rfc3339(),
        opened_by: budget.opened_by.clone(),
        source: budget.source.clone(),
        reason: budget.reason.clone(),
        closed_at: budget.closed_at.map(|closed_at| closed_at.to_rfc3339()),
        close_approval_request_id: close_request
            .filter(|_| budget.state == TaskBudgetState::Open)
            .map(|approval| public_task_budget_approval_id(&approval.id)),
    })
}

pub(super) fn approval_response(
    snapshot: &TaskBudgetApprovalSnapshot,
    now: DateTime<Utc>,
    state: &ServerState,
) -> Result<TaskBudgetApprovalHttpResponse> {
    let approval = &snapshot.approval;
    let status = approval.status_at(now);
    let pending_close = (approval.kind == TaskBudgetApprovalKind::Close
        && status == hubu_core::budget::TaskBudgetApprovalStatus::Pending)
        .then_some(approval);
    Ok(TaskBudgetApprovalHttpResponse {
        approval_request_id: public_task_budget_approval_id(&approval.id),
        kind: match approval.kind {
            TaskBudgetApprovalKind::Open => "task_budget_open",
            TaskBudgetApprovalKind::Close => "task_budget_close",
        },
        summary: approval_summary(approval),
        status: status.as_str(),
        review: TaskBudgetApprovalReviewHttpResponse {
            agent_id: registration_agent_pub_id(&approval.agent_id, state)?,
            task_id: approval.task_id.clone(),
            currency: approval.currency.to_string(),
            amount_limit: MoneyHttpResponse::new(approval.amount_limit, approval.currency),
            reason: approval.reason.clone(),
            requested_at: approval.requested_at.to_rfc3339(),
            requested_by: approval.requested_by.clone(),
            source: approval.source.clone(),
            expires_at: approval.expires_at.to_rfc3339(),
        },
        resolved_at: approval.resolved_at.map(|at| at.to_rfc3339()),
        resolved_by: approval.resolved_by.clone(),
        task_budget: snapshot
            .task_budget
            .as_ref()
            .map(|task_budget| task_budget_response(task_budget, pending_close, state))
            .transpose()?,
    })
}

/// One-line review wording for a task budget approval request.
pub(super) fn approval_summary(approval: &TaskBudgetApproval) -> String {
    let amount = MoneyHttpResponse::new(approval.amount_limit, approval.currency).display;
    let currency = approval.currency.to_string().to_uppercase();
    match approval.kind {
        TaskBudgetApprovalKind::Open => format!(
            "Task budget: open {currency} {amount} for task {}. This reserves budget and does not authorize spend.",
            approval.task_id
        ),
        TaskBudgetApprovalKind::Close => format!(
            "Task budget: close the {currency} {amount} task budget for task {} and return its unused amount to the agent budget. This does not authorize spend.",
            approval.task_id
        ),
    }
}

/// Structured status and code for task budget command failures.
pub(super) fn error_response(error: &anyhow::Error) -> Option<HttpResponse> {
    if let Some(rejected) = error.downcast_ref::<NotASpendApproval>() {
        return Some(HttpResponse {
            status: 400,
            body: json!({
                "error": error.to_string(),
                "error_code": "task_budget_approval_not_spend",
                "task_budget_approval_route": rejected.route,
            }),
        });
    }
    let (status, code) = match error.downcast_ref::<TaskBudgetError>()? {
        TaskBudgetError::InvalidTaskId | TaskBudgetError::AmountMustBePositive => {
            (400, "invalid_task_budget")
        }
        TaskBudgetError::NoAvailableBudget { .. } => (409, "no_available_budget"),
        TaskBudgetError::InsufficientUnallocatedBudget { .. } => {
            (409, "insufficient_unallocated_budget")
        }
        TaskBudgetError::OpenTaskBudgetConflict => (409, "task_budget_conflict"),
        TaskBudgetError::UnknownTaskBudget => (404, "unknown_task_budget"),
        TaskBudgetError::OutstandingHolds { .. } => (409, "task_budget_outstanding_authorizations"),
        TaskBudgetError::ProposalPending => (409, "task_budget_proposal_pending"),
        TaskBudgetError::UnknownApproval => (404, "unknown_task_budget_approval"),
        TaskBudgetError::ApprovalResolved { .. } => (409, "task_budget_approval_resolved"),
        TaskBudgetError::TaskBudgetNotOpen => (409, "task_budget_not_open"),
        TaskBudgetError::Storage(_) => return None,
    };
    Some(HttpResponse {
        status,
        body: json!({"error": error.to_string(), "error_code": code}),
    })
}
