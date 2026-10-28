//! HTTP transport for task-scoped budgets: open, close, list and show.
//!
//! Amounts are exact decimals only; task budgets have no legacy cent fields.
use super::*;
use hubu_core::app::TaskBudgetService;
use hubu_core::budget::{
    public_task_budget_id, CloseTaskBudgetRequest, OpenTaskBudgetRequest, TaskBudgetError,
    TaskBudgetSnapshot, TaskBudgetState,
};

pub(super) const OPEN_ROUTE: &str = "/task-budgets";
pub(super) const CLOSE_ROUTE: &str = "/task-budgets/close";
pub(super) const SHOW_ROUTE: &str = "/task-budgets/show";

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
}

#[derive(Debug, Serialize)]
struct TaskBudgetCommandHttpResponse {
    task_budget: TaskBudgetHttpResponse,
    idempotent_replay: bool,
}

#[derive(Debug, Serialize)]
struct ListTaskBudgetsHttpResponse {
    task_budgets: Vec<TaskBudgetHttpResponse>,
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
        task_budget: task_budget_response(&response.task_budget, state)?,
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
        task_budget: task_budget_response(&response.task_budget, state)?,
        idempotent_replay: response.replayed,
    }))
}

/// `GET /task-budgets[?agent_id=..][&all=true]`: open task budgets, or all.
pub(super) fn list(request: &HttpRequest, state: &ServerState) -> Result<Value> {
    let user = authenticated_user(state)?;
    let agent_id = request
        .query
        .get("agent_id")
        .map(|agent| resolve_agent_id_for_user(agent, &UserContext::new(user.id.clone()), state))
        .transpose()?;
    let snapshots = state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?
        .list_task_budgets(&user.id, agent_id.as_ref(), query_flag(request, "all"))?;
    let task_budgets = snapshots
        .iter()
        .map(|snapshot| task_budget_response(snapshot, state))
        .collect::<Result<_>>()?;
    Ok(to_json(ListTaskBudgetsHttpResponse { task_budgets }))
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
    drop(governance);
    let task_budget = snapshot
        .as_ref()
        .map(|snapshot| task_budget_response(snapshot, state))
        .transpose()?;
    Ok(to_json(ShowTaskBudgetHttpResponse { task_budget }))
}

pub(super) fn task_budget_response(
    snapshot: &TaskBudgetSnapshot,
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
    })
}

/// Structured status and code for task budget command failures.
pub(super) fn error_response(error: &anyhow::Error) -> Option<HttpResponse> {
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
        TaskBudgetError::Storage(_) => return None,
    };
    Some(HttpResponse {
        status,
        body: json!({"error": error.to_string(), "error_code": code}),
    })
}
