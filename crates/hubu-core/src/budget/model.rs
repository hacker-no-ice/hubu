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
    pub administrative_state: BudgetAdministrativeState,
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
            administrative_state: BudgetAdministrativeState::Active,
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

/// Canonical identity for one requested successor edge in a budget's version
/// chain.
///
/// Generated version ids and timestamps are intentionally excluded so a retry
/// after an ambiguous commit can recover the already-persisted successor. The
/// tuple encoding is unambiguous for free-form provenance strings and is
/// version-tagged so the canonical contract can evolve deliberately.
pub fn budget_update_request_fingerprint(
    budget_id: &BudgetId,
    expected_revision: u64,
    amount_limit_cents: i64,
    actor: &str,
    source: &str,
    reason: Option<&str>,
) -> String {
    let actor = actor.trim();
    let source = source.trim();
    let reason = reason.map(str::trim).filter(|reason| !reason.is_empty());
    let canonical = serde_json::to_vec(&(
        "hubu-budget-limit-update-v1",
        budget_id.to_string(),
        expected_revision,
        amount_limit_cents,
        actor,
        source,
        reason,
    ))
    .expect("canonical budget update fields are always JSON serializable");
    format!("sha256:{:x}", Sha256::digest(canonical))
}

/// Persisted administrative state of a logical budget.
///
/// Time and balance lifecycle are deliberately absent: they are derived by
/// [`evaluate_budget_availability`] from one injected instant and the current
/// immutable version plus logical balance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetAdministrativeState {
    /// The budget has not been administratively disabled.
    Active,
    /// The budget was administratively disabled.
    Revoked,
}

impl BudgetAdministrativeState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Revoked => "revoked",
        }
    }
}

/// Effective availability of a logical budget at one instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetAvailability {
    Revoked,
    Scheduled,
    Expired,
    Exhausted,
    Active,
}

impl BudgetAvailability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Revoked => "revoked",
            Self::Scheduled => "scheduled",
            Self::Expired => "expired",
            Self::Exhausted => "exhausted",
            Self::Active => "active",
        }
    }

    pub fn allows_reservation(self) -> bool {
        self == Self::Active
    }

    pub fn allows_limit_update(self) -> bool {
        matches!(self, Self::Scheduled | Self::Active | Self::Exhausted)
    }

    pub fn is_default_visible(self) -> bool {
        matches!(self, Self::Scheduled | Self::Active | Self::Exhausted)
    }
}

impl std::fmt::Display for BudgetAvailability {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BudgetEvaluationError {
    #[error("budget current version belongs to another logical budget")]
    VersionBudgetMismatch,
    #[error("budget snapshot does not contain the current version")]
    CurrentVersionMismatch,
    #[error("budget balance belongs to another logical budget")]
    BalanceBudgetMismatch,
    #[error("budget balance exceeds the representable range")]
    BalanceOverflow,
    #[error("budget remaining cache {cached_amount_cents} does not equal derived remaining {derived_amount_cents}")]
    RemainingBalanceMismatch {
        cached_amount_cents: i64,
        derived_amount_cents: i64,
    },
}

/// Evaluate one logical budget snapshot with fixed precedence.
///
/// Revocation dominates time, scheduling precedes expiration, and both time
/// states precede exhaustion. The half-open period is active at
/// `starting_at` and expired exactly at `ending_before`.
pub fn evaluate_budget_availability(
    budget: &Budget,
    current_version: &BudgetVersion,
    balance: &BudgetBalance,
    now: DateTime<Utc>,
) -> Result<BudgetAvailability, BudgetEvaluationError> {
    if current_version.budget_id != budget.id {
        return Err(BudgetEvaluationError::VersionBudgetMismatch);
    }
    if current_version.id != budget.current_version_id {
        return Err(BudgetEvaluationError::CurrentVersionMismatch);
    }
    if balance.budget_id != budget.id {
        return Err(BudgetEvaluationError::BalanceBudgetMismatch);
    }
    let derived_remaining = current_version
        .amount_limit_cents
        .checked_sub(balance.consumed_amount_cents)
        .and_then(|remaining| remaining.checked_sub(balance.frozen_amount_cents))
        .ok_or(BudgetEvaluationError::BalanceOverflow)?;
    if balance.remaining_amount_cents != derived_remaining {
        return Err(BudgetEvaluationError::RemainingBalanceMismatch {
            cached_amount_cents: balance.remaining_amount_cents,
            derived_amount_cents: derived_remaining,
        });
    }

    if budget.administrative_state == BudgetAdministrativeState::Revoked {
        Ok(BudgetAvailability::Revoked)
    } else if now < budget.period.starting_at {
        Ok(BudgetAvailability::Scheduled)
    } else if budget
        .period
        .ending_before
        .is_some_and(|ending_before| ending_before <= now)
    {
        Ok(BudgetAvailability::Expired)
    } else if derived_remaining <= 0 {
        Ok(BudgetAvailability::Exhausted)
    } else {
        Ok(BudgetAvailability::Active)
    }
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

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone};

    use super::*;

    fn lifecycle_snapshot(
        administrative_state: BudgetAdministrativeState,
        starting_at: DateTime<Utc>,
        ending_before: Option<DateTime<Utc>>,
        remaining_amount_cents: i64,
    ) -> (Budget, BudgetVersion, BudgetBalance) {
        let mut budget = Budget::new(
            BudgetId::new(),
            AgentId::new(),
            Currency::Usd,
            TimePeriod::new(starting_at, ending_before).unwrap(),
        );
        budget.administrative_state = administrative_state;
        let version = BudgetVersion::initial(&budget, 100, "actor", "test", None).unwrap();
        let balance = BudgetBalance {
            budget_id: budget.id.clone(),
            consumed_amount_cents: 100 - remaining_amount_cents,
            frozen_amount_cents: 0,
            remaining_amount_cents,
        };
        (budget, version, balance)
    }

    #[test]
    fn budget_update_fingerprint_is_stable_and_normalizes_provenance() {
        let budget_id: BudgetId = "00000000-0000-0000-0000-000000000001".parse().unwrap();
        let canonical = budget_update_request_fingerprint(
            &budget_id,
            7,
            12_345,
            "actor:user-1",
            "api",
            Some("quarterly increase"),
        );

        assert_eq!(
            canonical,
            "sha256:f88ff1fcc9cdf4e030b21aadacfc5cc079588a29df32fc12f9263b93bb43602a"
        );
        assert_eq!(
            canonical,
            budget_update_request_fingerprint(
                &budget_id,
                7,
                12_345,
                "  actor:user-1  ",
                " api ",
                Some(" quarterly increase "),
            )
        );
        assert_eq!(
            budget_update_request_fingerprint(&budget_id, 7, 12_345, "actor:user-1", "api", None,),
            budget_update_request_fingerprint(
                &budget_id,
                7,
                12_345,
                "actor:user-1",
                "api",
                Some("   "),
            )
        );
    }

    #[test]
    fn availability_uses_half_open_boundaries_and_fixed_precedence() {
        let start = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
        let end = start + Duration::hours(1);

        for (now, remaining, expected) in [
            (
                start - Duration::nanoseconds(1),
                100,
                BudgetAvailability::Scheduled,
            ),
            (
                start - Duration::nanoseconds(1),
                0,
                BudgetAvailability::Scheduled,
            ),
            (start, 100, BudgetAvailability::Active),
            (
                end - Duration::nanoseconds(1),
                100,
                BudgetAvailability::Active,
            ),
            (start, 0, BudgetAvailability::Exhausted),
            (start, -1, BudgetAvailability::Exhausted),
            (end, 100, BudgetAvailability::Expired),
            (end, 0, BudgetAvailability::Expired),
        ] {
            let (budget, version, balance) = lifecycle_snapshot(
                BudgetAdministrativeState::Active,
                start,
                Some(end),
                remaining,
            );
            assert_eq!(
                evaluate_budget_availability(&budget, &version, &balance, now).unwrap(),
                expected
            );
        }

        for (now, remaining) in [
            (start - Duration::nanoseconds(1), 100),
            (start, 0),
            (end, 100),
        ] {
            let (budget, version, balance) = lifecycle_snapshot(
                BudgetAdministrativeState::Revoked,
                start,
                Some(end),
                remaining,
            );
            assert_eq!(
                evaluate_budget_availability(&budget, &version, &balance, now).unwrap(),
                BudgetAvailability::Revoked
            );
        }
    }

    #[test]
    fn open_ended_availability_and_predicates_share_the_same_contract() {
        let start = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
        let (budget, version, balance) =
            lifecycle_snapshot(BudgetAdministrativeState::Active, start, None, 100);
        assert_eq!(
            evaluate_budget_availability(
                &budget,
                &version,
                &balance,
                start - Duration::nanoseconds(1),
            )
            .unwrap(),
            BudgetAvailability::Scheduled
        );
        assert_eq!(
            evaluate_budget_availability(&budget, &version, &balance, start).unwrap(),
            BudgetAvailability::Active
        );
        assert!(BudgetAvailability::Active.allows_reservation());
        assert!(!BudgetAvailability::Exhausted.allows_reservation());
        assert!(BudgetAvailability::Scheduled.allows_limit_update());
        assert!(BudgetAvailability::Exhausted.allows_limit_update());
        assert!(!BudgetAvailability::Expired.allows_limit_update());
        assert!(BudgetAvailability::Scheduled.is_default_visible());
        assert!(!BudgetAvailability::Revoked.is_default_visible());
    }

    #[test]
    fn availability_rejects_historical_or_incoherent_snapshots() {
        let start = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
        let (budget, mut version, mut balance) =
            lifecycle_snapshot(BudgetAdministrativeState::Active, start, None, 100);
        version.id = BudgetVersionId::new();
        assert_eq!(
            evaluate_budget_availability(&budget, &version, &balance, start),
            Err(BudgetEvaluationError::CurrentVersionMismatch)
        );

        version.id = budget.current_version_id.clone();
        balance.remaining_amount_cents = 99;
        assert!(matches!(
            evaluate_budget_availability(&budget, &version, &balance, start),
            Err(BudgetEvaluationError::RemainingBalanceMismatch { .. })
        ));
    }
}
