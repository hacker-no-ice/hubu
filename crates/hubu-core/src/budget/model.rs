use chrono::{DateTime, Utc};
use hubu_common::ids::{
    AgentId, BudgetHoldId, BudgetId, BudgetVersionId, SpendDecisionId, SpendExecutorClaimId,
};
use hubu_common::money::Currency;
use hubu_common::time::TimePeriod;
use sha2::{Digest, Sha256};

/// Stable logical spending allocation for one agent over a time period.
///
/// Agent, currency, and period are immutable properties of this logical
/// allocation. The limit is owned solely by the [`BudgetVersion`] named by
/// `current_version_id`.
#[derive(Debug, Clone)]
pub struct Budget {
    pub id: BudgetId,
    pub agent_id: AgentId,
    pub current_version_id: BudgetVersionId,
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
    pub fn new(id: BudgetId, agent_id: AgentId, currency: Currency, period: TimePeriod) -> Self {
        let created_at = Utc::now();

        Self {
            id,
            agent_id,
            current_version_id: BudgetVersionId::new(),
            currency,
            period,
            status: BudgetStatus::Active,
            created_at,
            updated_at: created_at,
        }
    }
}

/// Immutable configuration revision for a logical [`Budget`].
#[derive(Debug, Clone)]
pub struct BudgetVersion {
    pub id: BudgetVersionId,
    pub budget_id: BudgetId,
    pub revision: u64,
    pub predecessor_version_id: Option<BudgetVersionId>,
    pub amount_limit_cents: i64,
    pub effective_at: DateTime<Utc>,
    pub actor: String,
    pub source: String,
    pub reason: Option<String>,
    pub request_fingerprint: String,
    pub created_at: DateTime<Utc>,
}

impl BudgetVersion {
    pub fn initial(
        budget: &Budget,
        amount_limit_cents: i64,
        actor: impl Into<String>,
        source: impl Into<String>,
        reason: Option<String>,
    ) -> Result<Self, BudgetError> {
        if amount_limit_cents <= 0 {
            return Err(BudgetError::AmountLimitMustBePositive);
        }
        let actor = actor.into();
        let source = source.into();
        Ok(Self {
            id: budget.current_version_id.clone(),
            budget_id: budget.id.clone(),
            revision: 1,
            predecessor_version_id: None,
            amount_limit_cents,
            effective_at: budget.created_at,
            actor,
            source,
            reason,
            request_fingerprint: initial_budget_request_fingerprint(
                &budget.agent_id,
                amount_limit_cents,
                budget.currency,
                &budget.period,
            ),
            created_at: budget.created_at,
        })
    }
}

pub fn initial_budget_request_fingerprint(
    agent_id: &AgentId,
    amount_limit_cents: i64,
    currency: Currency,
    period: &TimePeriod,
) -> String {
    let canonical = format!(
        "agent_id={}\namount_limit_cents={}\ncurrency={}\nstarting_at={}\nending_before={}",
        agent_id,
        amount_limit_cents,
        currency,
        period.starting_at.to_rfc3339(),
        period
            .ending_before
            .map(|value| value.to_rfc3339())
            .unwrap_or_default(),
    );
    format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))
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
/// Consumed and frozen belong to the stable logical budget. Remaining is a
/// derived compatibility cache (`current version limit - consumed - frozen`),
/// validated whenever records are persisted or hydrated; it may legitimately
/// be negative after a human-confirmed provider overrun.
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
    pub budget_version_id: BudgetVersionId,
    pub spend_decision_id: SpendDecisionId,
    pub amount_cents: i64,
    pub currency: Currency,
    pub status: BudgetHoldStatus,
    pub executor_claim_id: Option<SpendExecutorClaimId>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetHoldError {
    CannotClaimNonFrozenHold,
    CannotSettleNonFrozenHold,
    CannotReleaseNonFrozenHold,
}

impl BudgetHold {
    pub fn claim(
        &mut self,
        claim_id: SpendExecutorClaimId,
        expires_at: DateTime<Utc>,
    ) -> Result<(), BudgetHoldError> {
        match &self.status {
            BudgetHoldStatus::Frozen => {
                self.status = BudgetHoldStatus::Claimed;
                self.executor_claim_id = Some(claim_id);
                self.expires_at = expires_at;
                self.updated_at = Utc::now();
                Ok(())
            }
            BudgetHoldStatus::Claimed if self.executor_claim_id.as_ref() == Some(&claim_id) => {
                Ok(())
            }
            _ => Err(BudgetHoldError::CannotClaimNonFrozenHold),
        }
    }

    pub fn settle(&mut self) -> Result<(), BudgetHoldError> {
        match &self.status {
            BudgetHoldStatus::Frozen | BudgetHoldStatus::Claimed => {
                self.status = BudgetHoldStatus::Settled;
                self.updated_at = Utc::now();
                Ok(())
            }
            _ => Err(BudgetHoldError::CannotSettleNonFrozenHold),
        }
    }

    pub fn release(&mut self) -> Result<(), BudgetHoldError> {
        match &self.status {
            BudgetHoldStatus::Frozen | BudgetHoldStatus::Claimed => {
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
    /// An executor has exclusively claimed the hold for billable work.
    Claimed,
    /// Payment settled and the amount moved into consumed usage.
    Settled,
    /// Hold was cancelled or unused and the amount returned to the budget.
    Released,
    /// Hold passed its expiration time before settlement.
    Expired,
}
