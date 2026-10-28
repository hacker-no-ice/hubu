//! Open and close task-scoped budgets against the agent's available budget.

use chrono::{DateTime, Utc};

use crate::budget::public_task_budget_id;
use crate::budget::{
    BudgetManager, CloseTaskBudgetRequest, CloseTaskBudgetResponse, OpenTaskBudgetRequest,
    OpenTaskBudgetResponse, TaskBudgetError,
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
        let parent = budgets
            .get_evaluated_budgets_by_agent_id(&request.agent_id, now)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?
            .into_iter()
            .find(|budget| {
                budget.current.budget.currency == request.currency
                    && budget.availability.allows_reservation()
            })
            .ok_or(TaskBudgetError::NoAvailableBudget {
                currency: request.currency,
            })?;
        let response = governance.open_task_budget(&request, &parent.current, now)?;
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
}
