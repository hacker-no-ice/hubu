use crate::budget::model::{
    BudgetAvailability, BudgetError, BudgetEvaluationError, BudgetHoldError,
};

#[derive(Debug, thiserror::Error)]
pub enum BudgetManagerError {
    #[error(transparent)]
    Storage(#[from] crate::storage::StorageError),
    #[error("invalid budget: {0:?}")]
    InvalidBudget(BudgetError),

    #[error("budget not found")]
    UnknownBudget,

    #[error("budget hold not found")]
    UnknownBudgetHold,

    #[error("budget balance not found")]
    MissingBudgetBalance,

    #[error("budget is unavailable: {0}")]
    BudgetUnavailable(BudgetAvailability),

    #[error("budget currency does not match request currency")]
    CurrencyMismatch,

    #[error("amount must be positive")]
    AmountMustBePositive,

    #[error("budget does not have enough remaining balance")]
    InsufficientRemainingBudget,

    #[error("budget is already revoked")]
    BudgetAlreadyRevoked,

    #[error("spend decision already has a budget hold")]
    DuplicateSpendDecisionHold,

    #[error("budget hold has expired")]
    ExpiredBudgetHold,

    #[error("invalid budget hold transition: {0:?}")]
    InvalidBudgetHoldTransition(BudgetHoldError),

    #[error("budget period overlaps an existing budget for the same agent and currency")]
    OverlappingBudgetPeriod,

    #[error("invalid persisted budget state: {0}")]
    InvalidPersistedState(String),

    #[error("budget version actor and source provenance are required")]
    MissingBudgetVersionProvenance,
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

impl From<BudgetEvaluationError> for BudgetManagerError {
    fn from(error: BudgetEvaluationError) -> Self {
        Self::InvalidPersistedState(error.to_string())
    }
}

use crate::storage::StorageError;
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum BudgetUpdateError {
    #[error("budget update amount must be positive")]
    AmountLimitMustBePositive,

    #[error("budget update expected_revision must be at least 1")]
    ExpectedRevisionMustBePositive,

    #[error("budget version actor is required")]
    MissingActor,

    #[error("budget version source is required")]
    MissingSource,

    #[error("budget not found")]
    UnknownBudget,

    #[error("revoked budget cannot be updated")]
    BudgetRevoked,

    #[error("expired budget cannot be updated")]
    BudgetExpired,

    #[error(
        "budget limit {requested_amount_cents} is below committed usage {committed_amount_cents}"
    )]
    LimitBelowCommitted {
        requested_amount_cents: i64,
        committed_amount_cents: i64,
    },

    #[error(
        "budget revision conflict: expected revision {expected_revision}, current revision is {current_revision}"
    )]
    RevisionConflict {
        expected_revision: u64,
        current_revision: u64,
    },

    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl From<rusqlite::Error> for BudgetUpdateError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.into())
    }
}
