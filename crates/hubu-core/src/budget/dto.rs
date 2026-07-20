use chrono::{DateTime, Utc};
use hubu_common::ids::{AgentId, BudgetId, SpendDecisionId};
use hubu_common::money::Currency;
use hubu_common::time::TimePeriod;

use crate::budget::model::{Budget, BudgetBalance, BudgetHold};

#[derive(Debug, Clone)]
pub struct CreateSingleBudgetRequest {
    pub agent_id: AgentId,
    pub amount_limit_cents: i64,
    pub currency: Currency,
    pub period: TimePeriod,
}

#[derive(Debug, Clone)]
pub struct CreateSingleBudgetResponse {
    pub budget: Budget,
    pub balance: BudgetBalance,
}

#[derive(Debug, Clone)]
pub struct CreateBudgetSeriesRequest {
    pub agent_id: AgentId,
    pub amount_limit_cents: i64,
    pub currency: Currency,
    pub starting_at: DateTime<Utc>,
    pub recurrence: BudgetRecurrence,
    pub period_count: usize,
}

#[derive(Debug, Clone)]
pub struct CreateBudgetSeriesResponse {
    pub budgets: Vec<BudgetWithBalance>,
}

#[derive(Debug, Clone, Copy)]
pub enum BudgetRecurrence {
    Daily,
    Monthly,
    Yearly,
}

#[derive(Debug, Clone)]
pub struct ReserveBudgetRequest {
    pub budget_id: BudgetId,
    pub spend_decision_id: SpendDecisionId,
    pub amount_cents: i64,
    pub currency: Currency,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ReserveBudgetResponse {
    pub hold: BudgetHold,
    pub balance: BudgetBalance,
}

#[derive(Debug, Clone)]
pub struct SettleBudgetResponse {
    pub hold: BudgetHold,
    pub balance: BudgetBalance,
}

#[derive(Debug, Clone)]
pub struct ReleaseBudgetResponse {
    pub hold: BudgetHold,
    pub balance: BudgetBalance,
}

#[derive(Debug, Clone)]
pub struct ExpireBudgetHoldResponse {
    pub hold: BudgetHold,
    pub balance: BudgetBalance,
}

#[derive(Debug, Clone)]
pub struct BudgetWithBalance {
    pub budget: Budget,
    pub balance: BudgetBalance,
}
