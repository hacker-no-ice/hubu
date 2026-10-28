//! Open and close task-scoped budgets against the agent's available budget.
//!
//! Humans open and close directly. Agents propose and request close; a human
//! approves or denies each request.

use chrono::{DateTime, Utc};

use crate::budget::{public_task_budget_approval_id, public_task_budget_id};
use crate::budget::{
    BudgetManager, BudgetWithBalance, CloseTaskBudgetRequest, CloseTaskBudgetResponse,
    OpenTaskBudgetRequest, OpenTaskBudgetResponse, ProposeTaskBudgetResponse,
    RequestTaskBudgetCloseRequest, RequestTaskBudgetCloseResponse,
    ResolveTaskBudgetApprovalRequest, ResolveTaskBudgetApprovalResponse, TaskBudgetApprovalKind,
    TaskBudgetError,
};
use crate::persistence::SqliteGovernanceRepository;
use crate::storage::StorageError;
use crate::telemetry::log_event;
use serde_json::json;

pub struct TaskBudgetService;

impl TaskBudgetService {
    /// Open a task budget drawn from the agent's available budget in the
    /// requested currency.
    ///
    /// Callers hold the budget manager before the governance repository, the
    /// same order spend reservation uses, so allocation checks and
    /// reservations cannot interleave.
    pub fn open(
        &self,
        request: OpenTaskBudgetRequest,
        budgets: &BudgetManager,
        governance: &mut SqliteGovernanceRepository,
        now: DateTime<Utc>,
    ) -> Result<OpenTaskBudgetResponse, TaskBudgetError> {
        let parent = available_parent(budgets, &request.agent_id, request.currency, now)?.ok_or(
            TaskBudgetError::NoAvailableBudget {
                currency: request.currency,
            },
        )?;
        let response = governance.open_task_budget(&request, &parent, now)?;
        if !response.replayed {
            let budget = &response.task_budget.budget;
            log_event(
                "info",
                "task_budget_opened",
                json!({
                    "task_budget_id": public_task_budget_id(&budget.id),
                    "agent_id": budget.agent_id.to_string(),
                    "budget_id": budget.parent_budget_id.to_string(),
                    "amount_limit": budget.amount_limit.display(),
                    "currency": budget.currency.to_string(),
                    "source": budget.source,
                }),
            );
        }
        Ok(response)
    }

    /// Close a task budget, returning its unused allocation to the agent budget.
    pub fn close(
        &self,
        request: CloseTaskBudgetRequest,
        governance: &mut SqliteGovernanceRepository,
        now: DateTime<Utc>,
    ) -> Result<CloseTaskBudgetResponse, TaskBudgetError> {
        let response = governance.close_task_budget(&request, now)?;
        if !response.replayed {
            let snapshot = &response.task_budget;
            log_event(
                "info",
                "task_budget_closed",
                json!({
                    "task_budget_id": public_task_budget_id(&snapshot.budget.id),
                    "agent_id": snapshot.budget.agent_id.to_string(),
                    "consumed_amount": snapshot.usage.consumed_amount.display(),
                    "returned_amount": snapshot.returned_amount().display(),
                    "source": request.source,
                }),
            );
        }
        Ok(response)
    }

    /// Record an agent's proposal to open a task budget. Nothing is reserved
    /// until a human approves it on the approval link.
    pub fn propose(
        &self,
        request: OpenTaskBudgetRequest,
        budgets: &BudgetManager,
        governance: &mut SqliteGovernanceRepository,
        now: DateTime<Utc>,
    ) -> Result<ProposeTaskBudgetResponse, TaskBudgetError> {
        let parent = available_parent(budgets, &request.agent_id, request.currency, now)?.ok_or(
            TaskBudgetError::NoAvailableBudget {
                currency: request.currency,
            },
        )?;
        let response = governance.propose_task_budget(&request, &parent, now)?;
        if let (Some(approval), false) = (&response.approval, response.replayed) {
            log_event(
                "info",
                "task_budget_proposed",
                json!({
                    "approval_request_id": public_task_budget_approval_id(&approval.id),
                    "agent_id": approval.agent_id.to_string(),
                    "amount_limit": approval.amount_limit.display(),
                    "currency": approval.currency.to_string(),
                    "source": approval.source,
                }),
            );
        }
        Ok(response)
    }

    /// Record an agent's request for a human to close a task budget.
    pub fn request_close(
        &self,
        request: RequestTaskBudgetCloseRequest,
        governance: &mut SqliteGovernanceRepository,
        now: DateTime<Utc>,
    ) -> Result<RequestTaskBudgetCloseResponse, TaskBudgetError> {
        let response = governance.request_task_budget_close(&request, now)?;
        if let (Some(approval), false) = (&response.approval, response.replayed) {
            log_event(
                "info",
                "task_budget_close_requested",
                json!({
                    "approval_request_id": public_task_budget_approval_id(&approval.id),
                    "task_budget_id": public_task_budget_id(&response.task_budget.budget.id),
                    "agent_id": approval.agent_id.to_string(),
                    "source": approval.source,
                }),
            );
        }
        Ok(response)
    }

    /// Apply a human's approve or deny decision to a task budget approval
    /// request. Callers hold the budget manager before the governance
    /// repository, because approving a proposal opens the task budget.
    pub fn resolve(
        &self,
        request: ResolveTaskBudgetApprovalRequest,
        budgets: &BudgetManager,
        governance: &mut SqliteGovernanceRepository,
        now: DateTime<Utc>,
    ) -> Result<ResolveTaskBudgetApprovalResponse, TaskBudgetError> {
        let parent = match governance
            .load_task_budget_approval_by_id(&request.owner_user_id, &request.approval_id)?
        {
            Some(approval) if approval.kind == TaskBudgetApprovalKind::Open => {
                available_parent(budgets, &approval.agent_id, approval.currency, now)?
            }
            _ => None,
        };
        let response = governance.resolve_task_budget_approval(&request, parent.as_ref(), now)?;
        if !response.replayed {
            let approval = &response.approval.approval;
            log_event(
                "info",
                "task_budget_approval_resolved",
                json!({
                    "approval_request_id": public_task_budget_approval_id(&approval.id),
                    "kind": approval.kind.as_str(),
                    "status": approval.status.as_str(),
                    "task_budget_id": approval.task_budget_id.as_ref().map(public_task_budget_id),
                    "agent_id": approval.agent_id.to_string(),
                }),
            );
        }
        Ok(response)
    }
}

/// The agent's budget in `currency` that can take reservations now.
fn available_parent(
    budgets: &BudgetManager,
    agent_id: &hubu_common::ids::AgentId,
    currency: hubu_common::money::Currency,
    now: DateTime<Utc>,
) -> Result<Option<BudgetWithBalance>, TaskBudgetError> {
    Ok(budgets
        .get_evaluated_budgets_by_agent_id(agent_id, now)
        .map_err(|error| StorageError::InvalidData(error.to_string()))?
        .into_iter()
        .find(|budget| {
            budget.current.budget.currency == currency && budget.availability.allows_reservation()
        })
        .map(|budget| budget.current))
}
