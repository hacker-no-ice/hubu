//! Task-scoped budgets: short-lived allocations drawn from an agent budget.
//!
//! A task budget does not own money. While open it reserves headroom inside
//! its parent agent budget, so other spend cannot use it, and it caps spend
//! that carries its trusted `task_id`. Reservations remain ordinary holds on
//! the parent budget, attributed to the task budget. Closing a task budget
//! ends the allocation, which returns the unused remainder to the parent.

use chrono::{DateTime, Utc};
use hubu_common::ids::{AgentId, BudgetId, TaskBudgetId, UserId};
use hubu_common::money::{Amount, Currency, ACCOUNTING_SCALE};
use sha2::{Digest, Sha256};

use crate::storage::StorageError;

/// Longest accepted trusted task correlation, matching the MCP operation
/// registry's `task_id` bound.
pub const MAX_TASK_ID_BYTES: usize = 512;

/// Persisted lifecycle of a task budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskBudgetState {
    Open,
    Closed,
}

impl TaskBudgetState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }
}

impl std::str::FromStr for TaskBudgetState {
    type Err = StorageError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "open" => Ok(Self::Open),
            "closed" => Ok(Self::Closed),
            other => Err(StorageError::InvalidData(format!(
                "unknown task budget state {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskBudget {
    pub id: TaskBudgetId,
    pub owner_user_id: UserId,
    pub agent_id: AgentId,
    pub parent_budget_id: BudgetId,
    pub task_id: String,
    pub currency: Currency,
    pub amount_limit: Amount,
    pub state: TaskBudgetState,
    pub opened_at: DateTime<Utc>,
    pub opened_by: String,
    pub source: String,
    pub reason: Option<String>,
    pub request_fingerprint: String,
    pub closed_at: Option<DateTime<Utc>>,
    pub closed_by: Option<String>,
}

/// Spend attributed to one task budget.
///
/// `frozen_amount` is the total of its outstanding (frozen or claimed) holds.
/// `consumed_amount` is the budget charge recorded in the ledger for its
/// settled holds, including corrections; a settled hold without recorded
/// charge evidence counts its full hold amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskBudgetUsage {
    pub frozen_amount: Amount,
    pub consumed_amount: Amount,
    pub outstanding_hold_count: u64,
    pub settled_hold_count: u64,
}

impl TaskBudgetUsage {
    pub const NONE: Self = Self {
        frozen_amount: Amount::ZERO,
        consumed_amount: Amount::ZERO,
        outstanding_hold_count: 0,
        settled_hold_count: 0,
    };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskBudgetSnapshot {
    pub budget: TaskBudget,
    pub usage: TaskBudgetUsage,
}

impl TaskBudgetSnapshot {
    /// `limit - frozen - consumed`. Negative only after a human-confirmed
    /// provider overrun.
    pub fn remaining_amount(&self) -> Amount {
        self.budget
            .amount_limit
            .checked_sub(self.usage.frozen_amount)
            .and_then(|remaining| remaining.checked_sub(self.usage.consumed_amount))
            .unwrap_or(Amount::ZERO)
    }

    /// Unused allocation an open task budget holds back from its parent.
    /// Closed task budgets hold nothing back.
    pub fn allocated_headroom(&self) -> Amount {
        if self.budget.state == TaskBudgetState::Closed {
            return Amount::ZERO;
        }
        let remaining = self.remaining_amount();
        if remaining.is_negative() {
            Amount::ZERO
        } else {
            remaining
        }
    }

    /// Unused allocation that returns to the parent when the task closes.
    pub fn returned_amount(&self) -> Amount {
        let remaining = self.remaining_amount();
        if remaining.is_negative() {
            Amount::ZERO
        } else {
            remaining
        }
    }
}

/// Trusted human or agent intent to open a task budget.
#[derive(Debug, Clone)]
pub struct OpenTaskBudgetRequest {
    pub owner_user_id: UserId,
    pub agent_id: AgentId,
    pub task_id: String,
    pub currency: Currency,
    pub amount_limit: Amount,
    pub actor: String,
    pub source: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OpenTaskBudgetResponse {
    pub task_budget: TaskBudgetSnapshot,
    /// True when an identical open request recovered the existing budget.
    pub replayed: bool,
}

#[derive(Debug, Clone)]
pub struct CloseTaskBudgetRequest {
    pub owner_user_id: UserId,
    pub task_budget_id: TaskBudgetId,
    pub actor: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct CloseTaskBudgetResponse {
    pub task_budget: TaskBudgetSnapshot,
    /// True when the task budget was already closed.
    pub replayed: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum TaskBudgetError {
    #[error("task_id must be 1 to {MAX_TASK_ID_BYTES} bytes without surrounding whitespace or control characters")]
    InvalidTaskId,
    #[error("task budget amount must be positive")]
    AmountMustBePositive,
    #[error("agent has no active {currency} budget to draw a task budget from")]
    NoAvailableBudget { currency: Currency },
    #[error("agent budget has only {available} unallocated; requested {requested}")]
    InsufficientUnallocatedBudget {
        available: Amount,
        requested: Amount,
    },
    #[error("an open task budget for this task already exists with a different limit")]
    OpenTaskBudgetConflict,
    #[error("unknown task budget")]
    UnknownTaskBudget,
    #[error("task budget has {count} outstanding authorization(s); settle or release them before closing")]
    OutstandingHolds { count: u64 },
    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl From<rusqlite::Error> for TaskBudgetError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.into())
    }
}

pub fn validate_task_id(task_id: &str) -> Result<(), TaskBudgetError> {
    if task_id.is_empty()
        || task_id.len() > MAX_TASK_ID_BYTES
        || task_id.trim() != task_id
        || task_id.chars().any(char::is_control)
    {
        return Err(TaskBudgetError::InvalidTaskId);
    }
    Ok(())
}

/// Canonical identity of one open request. Amounts use accounting units only.
pub fn task_budget_open_fingerprint(
    agent_id: &AgentId,
    task_id: &str,
    currency: Currency,
    amount_limit: Amount,
) -> String {
    let canonical = serde_json::to_vec(&(
        "hubu-task-budget-open-v1",
        agent_id.to_string(),
        task_id,
        currency.to_string(),
        amount_limit.units(),
        ACCOUNTING_SCALE,
    ))
    .expect("canonical task budget fields are always JSON serializable");
    format!("sha256:{:x}", Sha256::digest(canonical))
}

/// Public identifier for a task budget (`tbg_` plus the shared public suffix).
pub fn public_task_budget_id(id: &TaskBudgetId) -> String {
    format!("tbg_{}", id.public_suffix())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_money::cents;

    fn snapshot(
        limit: i64,
        frozen: i64,
        consumed: i64,
        state: TaskBudgetState,
    ) -> TaskBudgetSnapshot {
        TaskBudgetSnapshot {
            budget: TaskBudget {
                id: TaskBudgetId::new(),
                owner_user_id: UserId::new(),
                agent_id: AgentId::new(),
                parent_budget_id: BudgetId::new(),
                task_id: "design".into(),
                currency: Currency::Usd,
                amount_limit: cents(limit),
                state,
                opened_at: Utc::now(),
                opened_by: "actor".into(),
                source: "test".into(),
                reason: None,
                request_fingerprint: "sha256:x".into(),
                closed_at: None,
                closed_by: None,
            },
            usage: TaskBudgetUsage {
                frozen_amount: cents(frozen),
                consumed_amount: cents(consumed),
                ..TaskBudgetUsage::NONE
            },
        }
    }

    #[test]
    fn headroom_tracks_open_remaining_and_never_goes_negative() {
        let open = snapshot(300, 40, 100, TaskBudgetState::Open);
        assert_eq!(open.remaining_amount(), cents(160));
        assert_eq!(open.allocated_headroom(), cents(160));
        let overrun = snapshot(300, 0, 310, TaskBudgetState::Open);
        assert_eq!(overrun.remaining_amount(), cents(-10));
        assert_eq!(overrun.allocated_headroom(), Amount::ZERO);
        assert_eq!(overrun.returned_amount(), Amount::ZERO);
        let closed = snapshot(300, 0, 100, TaskBudgetState::Closed);
        assert_eq!(closed.allocated_headroom(), Amount::ZERO);
        assert_eq!(closed.returned_amount(), cents(200));
    }

    #[test]
    fn task_ids_are_trimmed_bounded_and_printable() {
        assert!(validate_task_id("linear:HUB-259").is_ok());
        for invalid in [
            "",
            " padded",
            "tab\there",
            &"x".repeat(MAX_TASK_ID_BYTES + 1),
        ] {
            assert!(matches!(
                validate_task_id(invalid),
                Err(TaskBudgetError::InvalidTaskId)
            ));
        }
    }

    #[test]
    fn open_fingerprint_uses_accounting_units() {
        let agent: AgentId = "00000000-0000-0000-0000-000000000001".parse().unwrap();
        let three = task_budget_open_fingerprint(&agent, "design", Currency::Usd, cents(300));
        assert_eq!(
            three,
            task_budget_open_fingerprint(
                &agent,
                "design",
                Currency::Usd,
                "3.000000".parse().unwrap()
            )
        );
        assert_ne!(
            three,
            task_budget_open_fingerprint(
                &agent,
                "design",
                Currency::Usd,
                "3.000001".parse().unwrap()
            )
        );
        assert_ne!(
            three,
            task_budget_open_fingerprint(&agent, "design", Currency::Usdc, cents(300))
        );
    }
}
