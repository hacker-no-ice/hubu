use crate::budget::model::{BudgetError, BudgetHoldError};

#[derive(Debug, thiserror::Error)]
pub enum BudgetManagerError {
    #[error("invalid budget: {0:?}")]
    InvalidBudget(BudgetError),

    #[error("budget not found")]
    UnknownBudget,

    #[error("budget hold not found")]
    UnknownBudgetHold,

    #[error("budget balance not found")]
    MissingBudgetBalance,

    #[error("budget is not active")]
    BudgetNotActive,

    #[error("budget period is not active")]
    BudgetPeriodInactive,

    #[error("budget currency does not match request currency")]
    CurrencyMismatch,

    #[error("amount must be positive")]
    AmountMustBePositive,

    #[error("budget does not have enough remaining balance")]
    InsufficientRemainingBudget,

    #[error("spend decision already has a budget hold")]
    DuplicateSpendDecisionHold,

    #[error("invalid budget hold transition: {0:?}")]
    InvalidBudgetHoldTransition(BudgetHoldError),

    #[error("recurring budget series must create at least one period")]
    EmptyBudgetSeries,

    #[error("budget recurrence could not produce the next period boundary")]
    InvalidRecurrenceBoundary,

    #[error("budget period overlaps an existing budget for the same scope and currency")]
    OverlappingBudgetPeriod,
}

impl From<BudgetError> for BudgetManagerError {
    fn from(error: BudgetError) -> Self {
        Self::InvalidBudget(error)
    }
}

impl From<BudgetHoldError> for BudgetManagerError {
    fn from(error: BudgetHoldError) -> Self {
        Self::InvalidBudgetHoldTransition(error)
    }
}
