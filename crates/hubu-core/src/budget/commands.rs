use super::BudgetUpdateError;
use hubu_common::ids::BudgetId;

/// Manager command for changing the total limit of one stable logical
/// budget. Transport ownership checks and public-id parsing remain outside this
/// facade.
#[derive(Debug, Clone)]
pub struct UpdateBudgetLimitRequest {
    pub budget_id: BudgetId,
    pub expected_revision: u64,
    pub amount_limit_cents: i64,
    pub actor: String,
    pub source: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum BudgetLimitUpdateError {
    #[error(transparent)]
    Append(#[from] BudgetUpdateError),
}

/// Result of an update or exact retry, including the current authoritative head.
#[derive(Debug, Clone)]
pub struct UpdateBudgetLimitResponse {
    pub applied_version: super::BudgetVersion,
    pub predecessor_revision: u64,
    pub current: super::BudgetWithBalance,
    pub idempotent_replay: bool,
}
