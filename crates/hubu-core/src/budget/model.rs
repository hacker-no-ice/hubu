use chrono::{DateTime, Utc};
use hubu_common::ids::{AgentId, BudgetHoldId, BudgetId, SpendDecisionId};
use hubu_common::money::Currency;
use hubu_common::time::TimePeriod;

/// Spending limit for one agent over a time period.
///
/// Budgets define the maximum amount that may be reserved or consumed for an
/// agent. The current balance can be tracked separately in [`BudgetBalance`] so
/// the limit remains immutable while usage changes over time.
#[derive(Debug, Clone)]
pub struct Budget {
    pub id: BudgetId,
    pub agent_id: AgentId,
    pub amount_limit_cents: i64,
    pub currency: Currency,
    pub period: TimePeriod,
    pub status: BudgetStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetError {
    AmountLimitMustBePositive,
}

impl Budget {
    pub fn new(
        id: BudgetId,
        agent_id: AgentId,
        amount_limit_cents: i64,
        currency: Currency,
        period: TimePeriod,
    ) -> Result<Self, BudgetError> {
        if amount_limit_cents <= 0 {
            return Err(BudgetError::AmountLimitMustBePositive);
        }

        let created_at = Utc::now();

        Ok(Self {
            id,
            agent_id,
            amount_limit_cents,
            currency,
            period,
            status: BudgetStatus::Active,
            created_at,
            updated_at: created_at,
        })
    }
}

/// Current lifecycle state of a budget.
#[derive(Debug, Clone)]
pub enum BudgetStatus {
    /// The budget may reserve new holds.
    Active,
    /// The budget has no remaining amount available.
    Exhausted,
    /// The budget's bounded time period has ended.
    Expired,
    /// The budget was disabled before the end of its period, if any.
    Revoked,
}

/// Cached usage totals for a budget.
///
/// These values make budget checks and reporting cheap. They can later be
/// rebuilt from spend decisions or ledger events if those become the source of
/// truth.
#[derive(Debug, Clone)]
pub struct BudgetBalance {
    pub budget_id: BudgetId,
    pub consumed_amount_cents: i64,
    pub frozen_amount_cents: i64,
    pub remaining_amount_cents: i64,
}

/// Reserved budget created from an approved spend decision.
///
/// A hold starts as [`BudgetHoldStatus::Frozen`] while payment authorization is
/// outstanding. Settlement consumes the hold; cancellation, denial, or unused
/// authorization releases it back to the budget.
#[derive(Debug, Clone)]
pub struct BudgetHold {
    pub id: BudgetHoldId,
    pub budget_id: BudgetId,
    pub spend_decision_id: SpendDecisionId,
    pub amount_cents: i64,
    pub currency: Currency,
    pub status: BudgetHoldStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetHoldError {
    CannotSettleNonFrozenHold,
    CannotReleaseNonFrozenHold,
}

impl BudgetHold {
    pub fn settle(&mut self) -> Result<(), BudgetHoldError> {
        match &self.status {
            BudgetHoldStatus::Frozen => {
                self.status = BudgetHoldStatus::Settled;
                self.updated_at = Utc::now();
                Ok(())
            }
            _ => Err(BudgetHoldError::CannotSettleNonFrozenHold),
        }
    }

    pub fn release(&mut self) -> Result<(), BudgetHoldError> {
        match &self.status {
            BudgetHoldStatus::Frozen => {
                self.status = BudgetHoldStatus::Released;
                self.updated_at = Utc::now();
                Ok(())
            }
            _ => Err(BudgetHoldError::CannotReleaseNonFrozenHold),
        }
    }
}

/// Lifecycle state for a reserved budget hold.
#[derive(Debug, Clone)]
pub enum BudgetHoldStatus {
    /// Amount is reserved and unavailable for other spend.
    Frozen,
    /// Payment settled and the amount moved into consumed usage.
    Settled,
    /// Hold was cancelled or unused and the amount returned to the budget.
    Released,
    /// Hold passed its expiration time before settlement.
    Expired,
}
