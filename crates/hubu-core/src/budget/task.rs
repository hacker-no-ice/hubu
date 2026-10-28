//! Task-scoped budgets: short-lived allocations drawn from an agent budget.
//!
//! A task budget does not own money. While open it reserves headroom inside
//! its parent agent budget, so other spend cannot use it, and it caps spend
//! that carries its trusted `task_id`. Reservations remain ordinary holds on
//! the parent budget, attributed to the task budget. Closing a task budget
//! ends the allocation, which returns the unused remainder to the parent.

use chrono::{DateTime, Duration, Utc};
use hubu_common::ids::{AgentId, BudgetId, TaskBudgetApprovalId, TaskBudgetId, UserId};
use hubu_common::money::{Amount, Currency, ACCOUNTING_SCALE};
use sha2::{Digest, Sha256};

use crate::storage::StorageError;

/// Longest accepted trusted task correlation, matching the MCP operation
/// registry's `task_id` bound.
pub const MAX_TASK_ID_BYTES: usize = 512;

/// How long an agent's proposal or close request waits for a human before
/// it expires. Task budgets themselves never expire.
pub const TASK_BUDGET_APPROVAL_TTL_HOURS: i64 = 24;

pub fn task_budget_approval_expires_at(requested_at: DateTime<Utc>) -> DateTime<Utc> {
    requested_at + Duration::hours(TASK_BUDGET_APPROVAL_TTL_HOURS)
}

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

/// What an agent asks a human to approve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskBudgetApprovalKind {
    /// Open a proposed task budget.
    Open,
    /// Close an open task budget.
    Close,
}

impl TaskBudgetApprovalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Close => "close",
        }
    }
}

impl std::str::FromStr for TaskBudgetApprovalKind {
    type Err = StorageError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "open" => Ok(Self::Open),
            "close" => Ok(Self::Close),
            other => Err(StorageError::InvalidData(format!(
                "unknown task budget approval kind {other}"
            ))),
        }
    }
}

/// Persisted state of a task budget approval request. A pending request
/// past its `expires_at` reads as expired before any sweep persists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskBudgetApprovalStatus {
    Pending,
    Approved,
    Denied,
    Expired,
}

impl TaskBudgetApprovalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Expired => "expired",
        }
    }
}

impl std::str::FromStr for TaskBudgetApprovalStatus {
    type Err = StorageError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            "denied" => Ok(Self::Denied),
            "expired" => Ok(Self::Expired),
            other => Err(StorageError::InvalidData(format!(
                "unknown task budget approval status {other}"
            ))),
        }
    }
}

/// An agent's request for a human to open or close a task budget.
///
/// An open request carries the proposed task, currency and limit; its
/// `task_budget_id` is set once a human approves it. A close request names
/// the open task budget it would close.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskBudgetApproval {
    pub id: TaskBudgetApprovalId,
    pub kind: TaskBudgetApprovalKind,
    pub owner_user_id: UserId,
    pub agent_id: AgentId,
    pub task_id: String,
    pub currency: Currency,
    pub amount_limit: Amount,
    pub task_budget_id: Option<TaskBudgetId>,
    pub status: TaskBudgetApprovalStatus,
    pub requested_at: DateTime<Utc>,
    pub requested_by: String,
    pub source: String,
    pub reason: Option<String>,
    pub request_fingerprint: String,
    pub expires_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by: Option<String>,
}

impl TaskBudgetApproval {
    /// Status as of `now`: a pending request past its expiry is expired.
    pub fn status_at(&self, now: DateTime<Utc>) -> TaskBudgetApprovalStatus {
        if self.status == TaskBudgetApprovalStatus::Pending && self.expires_at <= now {
            TaskBudgetApprovalStatus::Expired
        } else {
            self.status
        }
    }
}

/// An approval request together with the task budget it opened or would close.
#[derive(Debug, Clone)]
pub struct TaskBudgetApprovalSnapshot {
    pub approval: TaskBudgetApproval,
    pub task_budget: Option<TaskBudgetSnapshot>,
}

/// Outcome of an agent's proposal to open a task budget (an
/// [`OpenTaskBudgetRequest`] that waits for a human). Nothing is reserved
/// until a human approves it.
#[derive(Debug, Clone)]
pub struct ProposeTaskBudgetResponse {
    /// The pending proposal, unless an identical task budget is already open.
    pub approval: Option<TaskBudgetApproval>,
    /// The identical task budget that is already open, if any.
    pub task_budget: Option<TaskBudgetSnapshot>,
    /// True when the request recovered an existing proposal or open budget.
    pub replayed: bool,
}

/// An agent's request for a human to close an open task budget.
#[derive(Debug, Clone)]
pub struct RequestTaskBudgetCloseRequest {
    pub owner_user_id: UserId,
    pub task_budget_id: TaskBudgetId,
    pub actor: String,
    pub source: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RequestTaskBudgetCloseResponse {
    pub task_budget: TaskBudgetSnapshot,
    /// The pending close request; absent when the budget is already closed.
    pub approval: Option<TaskBudgetApproval>,
    /// True when the request recovered a pending close request or a closed budget.
    pub replayed: bool,
}

/// A human decision on a task budget approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskBudgetApprovalDecision {
    Approve,
    Deny,
}

#[derive(Debug, Clone)]
pub struct ResolveTaskBudgetApprovalRequest {
    pub owner_user_id: UserId,
    pub approval_id: TaskBudgetApprovalId,
    pub decision: TaskBudgetApprovalDecision,
    pub actor: String,
}

#[derive(Debug, Clone)]
pub struct ResolveTaskBudgetApprovalResponse {
    pub approval: TaskBudgetApprovalSnapshot,
    /// True when the request had already been resolved the same way.
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
    #[error("a proposal for this task is waiting for human approval")]
    ProposalPending,
    #[error("unknown task budget approval request")]
    UnknownApproval,
    #[error("task budget approval request is already {status}")]
    ApprovalResolved { status: &'static str },
    #[error("task budget is not open")]
    TaskBudgetNotOpen,
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

/// Prefix that marks a task budget approval request on the shared approval link.
pub const TASK_BUDGET_APPROVAL_ID_PREFIX: &str = "tba_";

/// Public identifier for a task budget approval request (`tba_` plus suffix).
pub fn public_task_budget_approval_id(id: &TaskBudgetApprovalId) -> String {
    format!("{TASK_BUDGET_APPROVAL_ID_PREFIX}{}", id.public_suffix())
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
