use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, Months, Utc};
use hubu_common::ids::{
    AgentId, BudgetHoldId, BudgetId, BudgetVersionId, SpendDecisionId, SpendExecutorClaimId,
};
use hubu_common::money::Currency;
use hubu_common::time::TimePeriod;
use serde_json::json;

use crate::budget::dto::{
    BudgetRecurrence, BudgetWithBalance, CreateBudgetSeriesRequest, CreateBudgetSeriesResponse,
    CreateSingleBudgetRequest, CreateSingleBudgetResponse, EvaluatedBudget,
    ExpireBudgetHoldResponse, ReleaseBudgetResponse, ReserveBudgetRequest, ReserveBudgetResponse,
    SettleBudgetResponse,
};
use crate::budget::error::BudgetManagerError;
use crate::budget::model::{
    Budget, BudgetAdministrativeState, BudgetBalance, BudgetHold, BudgetHoldStatus, BudgetVersion,
};
use crate::telemetry::log_event;

#[derive(Debug, Clone)]
pub struct BudgetVersionProvenance {
    pub actor: String,
    pub source: String,
    pub reason: Option<String>,
}

impl BudgetVersionProvenance {
    pub fn new(actor: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            actor: actor.into(),
            source: source.into(),
            reason: None,
        }
    }
}

impl Default for BudgetVersionProvenance {
    fn default() -> Self {
        Self::new("system:compatibility", "budget_manager")
    }
}

#[derive(Debug)]
pub struct BudgetManager {
    budgets: HashMap<BudgetId, Budget>,
    budget_versions: HashMap<BudgetVersionId, BudgetVersion>,
    budget_version_id_by_revision: HashMap<(BudgetId, u64), BudgetVersionId>,
    successor_version_id_by_predecessor: HashMap<BudgetVersionId, BudgetVersionId>,
    budget_balances: HashMap<BudgetId, BudgetBalance>,
    budget_holds: HashMap<BudgetHoldId, BudgetHold>,
    hold_id_by_spend_decision: HashMap<SpendDecisionId, BudgetHoldId>,

    budget_ids_by_agent_id: HashMap<AgentId, Vec<BudgetId>>,
}

impl BudgetManager {
    pub fn new() -> Self {
        Self {
            budgets: HashMap::new(),
            budget_versions: HashMap::new(),
            budget_version_id_by_revision: HashMap::new(),
            successor_version_id_by_predecessor: HashMap::new(),
            budget_balances: HashMap::new(),
            budget_holds: HashMap::new(),
            hold_id_by_spend_decision: HashMap::new(),
            budget_ids_by_agent_id: HashMap::new(),
        }
    }

    pub fn from_records(
        budgets: Vec<Budget>,
        versions: Vec<BudgetVersion>,
        balances: Vec<BudgetBalance>,
        holds: Vec<BudgetHold>,
    ) -> Result<Self, BudgetManagerError> {
        let mut manager = Self::new();
        for version in versions {
            if version.amount_limit_cents <= 0
                || version.revision == 0
                || version.actor.trim().is_empty()
                || version.source.trim().is_empty()
                || version.request_fingerprint.trim().is_empty()
            {
                return Err(invalid_persisted_budget_state(format!(
                    "budget version {} has invalid immutable metadata",
                    version.id
                )));
            }
            if manager
                .budget_versions
                .insert(version.id.clone(), version)
                .is_some()
            {
                return Err(invalid_persisted_budget_state(
                    "duplicate budget version id",
                ));
            }
        }
        for budget in budgets {
            let current_version = manager
                .budget_versions
                .get(&budget.current_version_id)
                .ok_or_else(|| {
                    invalid_persisted_budget_state(format!(
                        "budget {} has no current version",
                        budget.id
                    ))
                })?;
            if current_version.budget_id != budget.id {
                return Err(invalid_persisted_budget_state(format!(
                    "budget {} points at a version owned by another budget",
                    budget.id
                )));
            }
            if manager.budgets.contains_key(&budget.id) {
                return Err(invalid_persisted_budget_state("duplicate budget id"));
            }
            manager.index_budget(&budget);
            manager.budgets.insert(budget.id.clone(), budget);
        }

        let mut logical_budgets = manager.budgets.values().collect::<Vec<_>>();
        logical_budgets.sort_by_key(|budget| budget.id.to_string());
        for (index, left) in logical_budgets.iter().enumerate() {
            if left.administrative_state == BudgetAdministrativeState::Revoked {
                continue;
            }
            for right in logical_budgets.iter().skip(index + 1) {
                if right.administrative_state != BudgetAdministrativeState::Revoked
                    && left.agent_id == right.agent_id
                    && left.currency == right.currency
                    && periods_overlap(&left.period, &right.period)
                {
                    return Err(invalid_persisted_budget_state(format!(
                        "non-revoked budgets {} and {} overlap for one agent and currency",
                        left.id, right.id
                    )));
                }
            }
        }

        let mut revisions = HashSet::new();
        let mut predecessors = HashSet::new();
        let mut highest_revision_by_budget = HashMap::<BudgetId, u64>::new();
        for version in manager.budget_versions.values() {
            if !manager.budgets.contains_key(&version.budget_id) {
                return Err(invalid_persisted_budget_state(format!(
                    "budget version {} has no logical budget",
                    version.id
                )));
            }
            if !revisions.insert((version.budget_id.clone(), version.revision)) {
                return Err(invalid_persisted_budget_state(format!(
                    "budget {} has duplicate revision {}",
                    version.budget_id, version.revision
                )));
            }
            manager.budget_version_id_by_revision.insert(
                (version.budget_id.clone(), version.revision),
                version.id.clone(),
            );
            match (&version.predecessor_version_id, version.revision) {
                (None, 1) => {}
                (Some(predecessor_id), revision) if revision > 1 => {
                    let predecessor =
                        manager.budget_versions.get(predecessor_id).ok_or_else(|| {
                            invalid_persisted_budget_state(format!(
                                "budget version {} has no predecessor",
                                version.id
                            ))
                        })?;
                    if predecessor.budget_id != version.budget_id
                        || predecessor.revision.checked_add(1) != Some(version.revision)
                    {
                        return Err(invalid_persisted_budget_state(format!(
                            "budget version {} has an invalid predecessor chain",
                            version.id
                        )));
                    }
                    if !predecessors.insert((version.budget_id.clone(), predecessor_id.clone())) {
                        return Err(invalid_persisted_budget_state(format!(
                            "budget version {} has multiple successors",
                            predecessor_id
                        )));
                    }
                    manager
                        .successor_version_id_by_predecessor
                        .insert(predecessor_id.clone(), version.id.clone());
                }
                _ => {
                    return Err(invalid_persisted_budget_state(format!(
                        "budget version {} has an invalid root revision",
                        version.id
                    )));
                }
            }
            highest_revision_by_budget
                .entry(version.budget_id.clone())
                .and_modify(|revision| *revision = (*revision).max(version.revision))
                .or_insert(version.revision);
        }

        for budget in manager.budgets.values() {
            let current = &manager.budget_versions[&budget.current_version_id];
            if highest_revision_by_budget.get(&budget.id) != Some(&current.revision) {
                return Err(invalid_persisted_budget_state(format!(
                    "budget {} does not point at its latest version",
                    budget.id
                )));
            }
        }

        for balance in balances {
            let budget = manager.budgets.get(&balance.budget_id).ok_or_else(|| {
                invalid_persisted_budget_state(format!(
                    "balance references unknown budget {}",
                    balance.budget_id
                ))
            })?;
            if balance.consumed_amount_cents < 0 || balance.frozen_amount_cents < 0 {
                return Err(invalid_persisted_budget_state(format!(
                    "budget {} has a negative consumed or frozen balance",
                    balance.budget_id
                )));
            }
            let amount_limit_cents =
                manager.budget_versions[&budget.current_version_id].amount_limit_cents;
            let derived_remaining = amount_limit_cents
                .checked_sub(balance.consumed_amount_cents)
                .and_then(|value| value.checked_sub(balance.frozen_amount_cents))
                .ok_or_else(|| {
                    invalid_persisted_budget_state(format!(
                        "budget {} balance exceeds the representable range",
                        balance.budget_id
                    ))
                })?;
            if balance.remaining_amount_cents != derived_remaining {
                return Err(invalid_persisted_budget_state(format!(
                    "budget {} remaining balance does not derive from its current version",
                    balance.budget_id
                )));
            }
            if manager
                .budget_balances
                .insert(balance.budget_id.clone(), balance)
                .is_some()
            {
                return Err(invalid_persisted_budget_state(
                    "duplicate logical budget balance",
                ));
            }
        }
        for budget_id in manager.budgets.keys() {
            if !manager.budget_balances.contains_key(budget_id) {
                return Err(invalid_persisted_budget_state(format!(
                    "budget {budget_id} has no logical balance"
                )));
            }
        }
        let mut frozen_by_budget = HashMap::<BudgetId, i64>::new();
        for hold in holds {
            let version = manager
                .budget_versions
                .get(&hold.budget_version_id)
                .ok_or_else(|| {
                    invalid_persisted_budget_state(format!(
                        "hold {} references an unknown budget version",
                        hold.id
                    ))
                })?;
            let budget = manager.budgets.get(&hold.budget_id);
            if version.budget_id != hold.budget_id
                || !manager.budget_balances.contains_key(&hold.budget_id)
                || budget.is_none_or(|budget| budget.currency != hold.currency)
                || hold.amount_cents <= 0
            {
                return Err(invalid_persisted_budget_state(format!(
                    "hold {} has mismatched logical budget attribution",
                    hold.id
                )));
            }
            if manager
                .hold_id_by_spend_decision
                .insert(hold.spend_decision_id.clone(), hold.id.clone())
                .is_some()
            {
                return Err(invalid_persisted_budget_state(
                    "multiple holds reference one spend decision",
                ));
            }
            if matches!(
                &hold.status,
                BudgetHoldStatus::Frozen | BudgetHoldStatus::Claimed
            ) {
                let frozen_amount = frozen_by_budget.entry(hold.budget_id.clone()).or_default();
                *frozen_amount = frozen_amount
                    .checked_add(hold.amount_cents)
                    .ok_or_else(|| {
                        invalid_persisted_budget_state(format!(
                            "budget {} frozen holds exceed the representable range",
                            hold.budget_id
                        ))
                    })?;
            }
            if manager.budget_holds.insert(hold.id.clone(), hold).is_some() {
                return Err(invalid_persisted_budget_state("duplicate budget hold id"));
            }
        }
        for (budget_id, balance) in &manager.budget_balances {
            if frozen_by_budget.get(budget_id).copied().unwrap_or_default()
                != balance.frozen_amount_cents
            {
                return Err(invalid_persisted_budget_state(format!(
                    "budget {budget_id} frozen balance does not match its active holds"
                )));
            }
        }
        Ok(manager)
    }

    pub fn apply_persisted_finalization(&mut self, hold: BudgetHold, balance: BudgetBalance) {
        let budget_id = hold.budget_id.clone();
        self.budget_holds.insert(hold.id.clone(), hold);
        self.budget_balances.insert(budget_id, balance);
    }

    /// Apply a repository-authoritative version append after its transaction
    /// has committed.
    ///
    /// This operation is intentionally infallible: the SQLite repository has
    /// already validated lineage, balance, and current-pointer ownership. An
    /// exact retry may name an older `applied_version` while `current` points at
    /// a later head, so the manager indexes the immutable applied successor but
    /// never moves its logical head backward.
    pub fn apply_persisted_budget_version_append(
        &mut self,
        applied_version: BudgetVersion,
        current: BudgetWithBalance,
    ) {
        self.index_persisted_budget_version(applied_version);
        self.index_persisted_budget_version(current.version.clone());

        let current_revision = current.version.revision;
        let local_revision = self
            .budgets
            .get(&current.budget.id)
            .and_then(|budget| self.budget_versions.get(&budget.current_version_id))
            .map(|version| version.revision);
        if local_revision.is_some_and(|revision| revision > current_revision) {
            return;
        }

        debug_assert_eq!(current.budget.id, current.version.budget_id);
        debug_assert_eq!(current.budget.current_version_id, current.version.id);
        debug_assert_eq!(current.budget.id, current.balance.budget_id);
        debug_assert_eq!(
            current.balance.remaining_amount_cents,
            current
                .version
                .amount_limit_cents
                .checked_sub(current.balance.consumed_amount_cents)
                .and_then(|value| value.checked_sub(current.balance.frozen_amount_cents))
                .expect("persisted budget balance must remain representable")
        );
        debug_assert_eq!(
            current.balance.frozen_amount_cents,
            self.budget_holds
                .values()
                .filter(|hold| {
                    hold.budget_id == current.budget.id
                        && matches!(
                            hold.status,
                            BudgetHoldStatus::Frozen | BudgetHoldStatus::Claimed
                        )
                })
                .map(|hold| hold.amount_cents)
                .sum::<i64>()
        );

        if !self.budgets.contains_key(&current.budget.id) {
            self.index_budget(&current.budget);
        }
        self.budget_balances
            .insert(current.budget.id.clone(), current.balance);
        self.budgets
            .insert(current.budget.id.clone(), current.budget);
    }

    /// Create one budget and initialize its cached balance.
    ///
    /// An agent may only have one budget for a currency at any point in time.
    /// Creation rejects periods that overlap an existing budget with the same
    /// agent and currency.
    #[cfg(test)]
    pub fn create_single_budget(
        &mut self,
        request: CreateSingleBudgetRequest,
    ) -> Result<CreateSingleBudgetResponse, BudgetManagerError> {
        self.create_single_budget_with_provenance(request, BudgetVersionProvenance::default())
    }

    /// Create one logical budget, its immutable revision 1, and logical balance.
    ///
    /// Production callers must provide authenticated actor and source
    /// provenance for the version audit record.
    pub fn create_single_budget_with_provenance(
        &mut self,
        request: CreateSingleBudgetRequest,
        provenance: BudgetVersionProvenance,
    ) -> Result<CreateSingleBudgetResponse, BudgetManagerError> {
        self.create_budget_for_period(
            request.agent_id,
            request.amount_limit_cents,
            request.currency,
            request.period,
            &provenance,
        )
    }

    /// Create a finite recurring budget series.
    ///
    /// Each generated budget has its own balance. Consecutive periods use
    /// half-open boundaries, so the end of one period is the start of the next.
    /// The series is rejected if any generated period overlaps an existing
    /// budget for the same agent and currency.
    #[cfg(test)]
    pub fn create_budget_series(
        &mut self,
        request: CreateBudgetSeriesRequest,
    ) -> Result<CreateBudgetSeriesResponse, BudgetManagerError> {
        self.create_budget_series_with_provenance(request, BudgetVersionProvenance::default())
    }

    /// Create a finite recurring series with an independently versioned logical
    /// budget and balance for each period.
    pub fn create_budget_series_with_provenance(
        &mut self,
        request: CreateBudgetSeriesRequest,
        provenance: BudgetVersionProvenance,
    ) -> Result<CreateBudgetSeriesResponse, BudgetManagerError> {
        if request.period_count == 0 {
            log_event(
                "warn",
                "budget_series_create_rejected",
                json!({
                    "reason": "empty_budget_series",
                    "agent_id": request.agent_id.to_string(),
                    "amount_limit_cents": request.amount_limit_cents,
                    "currency": request.currency.to_string(),
                }),
            );
            return Err(BudgetManagerError::EmptyBudgetSeries);
        }

        let mut periods = Vec::with_capacity(request.period_count);
        let mut starting_at = request.starting_at;

        for _ in 0..request.period_count {
            let ending_before = next_period_boundary(starting_at, request.recurrence)?;
            let period = TimePeriod::new(starting_at, Some(ending_before))
                .expect("next period boundary should always be after period start");
            periods.push(period);
            starting_at = ending_before;
        }

        if periods
            .iter()
            .any(|period| self.has_overlapping_budget(&request.agent_id, request.currency, period))
        {
            log_event(
                "warn",
                "budget_series_create_rejected",
                json!({
                    "reason": "overlapping_budget_period",
                    "agent_id": request.agent_id.to_string(),
                    "amount_limit_cents": request.amount_limit_cents,
                    "currency": request.currency.to_string(),
                    "period_count": request.period_count,
                }),
            );
            return Err(BudgetManagerError::OverlappingBudgetPeriod);
        }

        let mut budgets = Vec::with_capacity(request.period_count);
        for period in periods {
            let budget_with_balance = build_budget_for_period(
                request.agent_id.clone(),
                request.amount_limit_cents,
                request.currency,
                period,
                &provenance,
            )?;
            budgets.push(budget_with_balance);
        }

        for budget_with_balance in &budgets {
            self.insert_budget(budget_with_balance);
        }

        log_event(
            "info",
            "budget_series_created",
            json!({
                "agent_id": request.agent_id.to_string(),
                "amount_limit_cents": request.amount_limit_cents,
                "currency": request.currency.to_string(),
                "period_count": budgets.len(),
            }),
        );
        Ok(CreateBudgetSeriesResponse { budgets })
    }

    /// Reserve budget for an approved spend decision.
    ///
    /// This method checks remaining balance and updates the hold and balance in
    /// one mutable operation. Wrap the manager in a process-level lock when it is
    /// shared across concurrent handlers.
    pub fn reserve_budget(
        &mut self,
        request: ReserveBudgetRequest,
    ) -> Result<ReserveBudgetResponse, BudgetManagerError> {
        self.reserve_budget_at(request, Utc::now())
    }

    /// Reserve from a budget that is effectively active at `now`.
    pub fn reserve_budget_at(
        &mut self,
        request: ReserveBudgetRequest,
        now: DateTime<Utc>,
    ) -> Result<ReserveBudgetResponse, BudgetManagerError> {
        if request.amount_cents <= 0 {
            log_budget_reservation_rejected(&request, "amount_must_be_positive");
            return Err(BudgetManagerError::AmountMustBePositive);
        }

        if self
            .hold_id_by_spend_decision
            .contains_key(&request.spend_decision_id)
        {
            log_budget_reservation_rejected(&request, "duplicate_spend_decision_hold");
            return Err(BudgetManagerError::DuplicateSpendDecisionHold);
        }

        let current = self.get_budget_by_id(&request.budget_id).ok_or_else(|| {
            log_budget_reservation_rejected(&request, "unknown_budget");
            BudgetManagerError::UnknownBudget
        })?;
        let availability = current.availability_at(now)?;
        if !availability.allows_reservation() {
            log_budget_reservation_rejected(&request, availability.as_str());
            return Err(BudgetManagerError::BudgetUnavailable(availability));
        }
        let budget = self
            .budgets
            .get(&request.budget_id)
            .expect("evaluated budget must remain indexed");
        let version = self
            .budget_versions
            .get(&budget.current_version_id)
            .ok_or(BudgetManagerError::UnknownBudget)?;

        if budget.currency != request.currency {
            log_budget_reservation_rejected(&request, "currency_mismatch");
            return Err(BudgetManagerError::CurrencyMismatch);
        }

        let balance = self
            .budget_balances
            .get_mut(&request.budget_id)
            .ok_or_else(|| {
                log_budget_reservation_rejected(&request, "missing_budget_balance");
                BudgetManagerError::MissingBudgetBalance
            })?;

        if balance.remaining_amount_cents < request.amount_cents {
            log_budget_reservation_rejected(&request, "insufficient_remaining_budget");
            return Err(BudgetManagerError::InsufficientRemainingBudget);
        }

        balance.remaining_amount_cents -= request.amount_cents;
        balance.frozen_amount_cents += request.amount_cents;

        let hold = BudgetHold {
            id: BudgetHoldId::new(),
            budget_id: request.budget_id,
            budget_version_id: version.id.clone(),
            spend_decision_id: request.spend_decision_id,
            amount_cents: request.amount_cents,
            currency: request.currency,
            status: BudgetHoldStatus::Frozen,
            executor_claim_id: None,
            created_at: now,
            updated_at: now,
            expires_at: request.expires_at,
        };

        self.hold_id_by_spend_decision
            .insert(hold.spend_decision_id.clone(), hold.id.clone());
        self.budget_holds.insert(hold.id.clone(), hold.clone());

        log_event(
            "info",
            "budget_reserved",
            json!({
                "budget_id": hold.budget_id.to_string(),
                "budget_version_id": hold.budget_version_id.to_string(),
                "hold_id": hold.id.to_string(),
                "spend_decision_id": hold.spend_decision_id.to_string(),
                "amount_cents": hold.amount_cents,
                "currency": hold.currency.to_string(),
                "consumed_amount_cents": balance.consumed_amount_cents,
                "frozen_amount_cents": balance.frozen_amount_cents,
                "remaining_amount_cents": balance.remaining_amount_cents,
                "expires_at": hold.expires_at.to_rfc3339(),
            }),
        );
        Ok(ReserveBudgetResponse {
            hold,
            balance: balance.clone(),
        })
    }

    /// Bind a frozen hold to one executor claim and extend its execution lease.
    pub fn claim_budget(
        &mut self,
        hold_id: &BudgetHoldId,
        claim_id: SpendExecutorClaimId,
        expires_at: DateTime<Utc>,
    ) -> Result<ReserveBudgetResponse, BudgetManagerError> {
        let hold = self
            .budget_holds
            .get_mut(hold_id)
            .ok_or(BudgetManagerError::UnknownBudgetHold)?;
        hold.claim(claim_id, expires_at)?;
        let balance = self
            .budget_balances
            .get(&hold.budget_id)
            .cloned()
            .ok_or(BudgetManagerError::MissingBudgetBalance)?;
        Ok(ReserveBudgetResponse {
            hold: hold.clone(),
            balance,
        })
    }

    /// Settle a frozen budget hold after payment succeeds.
    pub fn settle_budget(
        &mut self,
        hold_id: &BudgetHoldId,
    ) -> Result<SettleBudgetResponse, BudgetManagerError> {
        let hold = self.budget_holds.get_mut(hold_id).ok_or_else(|| {
            log_event(
                "warn",
                "budget_settle_rejected",
                json!({
                    "reason": "unknown_budget_hold",
                    "hold_id": hold_id.to_string(),
                }),
            );
            BudgetManagerError::UnknownBudgetHold
        })?;

        if hold.expires_at <= Utc::now() {
            log_event(
                "warn",
                "budget_settle_rejected",
                json!({
                    "reason": "expired_budget_hold",
                    "budget_id": hold.budget_id.to_string(),
                    "hold_id": hold.id.to_string(),
                    "spend_decision_id": hold.spend_decision_id.to_string(),
                    "expires_at": hold.expires_at.to_rfc3339(),
                }),
            );
            return Err(BudgetManagerError::ExpiredBudgetHold);
        }

        let balance = self
            .budget_balances
            .get_mut(&hold.budget_id)
            .ok_or_else(|| {
                log_event(
                    "warn",
                    "budget_settle_rejected",
                    json!({
                        "reason": "missing_budget_balance",
                        "budget_id": hold.budget_id.to_string(),
                        "hold_id": hold.id.to_string(),
                    }),
                );
                BudgetManagerError::MissingBudgetBalance
            })?;

        hold.settle()?;
        balance.frozen_amount_cents -= hold.amount_cents;
        balance.consumed_amount_cents += hold.amount_cents;

        log_event(
            "info",
            "budget_settled",
            json!({
                "budget_id": hold.budget_id.to_string(),
                "hold_id": hold.id.to_string(),
                "spend_decision_id": hold.spend_decision_id.to_string(),
                "amount_cents": hold.amount_cents,
                "consumed_amount_cents": balance.consumed_amount_cents,
                "frozen_amount_cents": balance.frozen_amount_cents,
                "remaining_amount_cents": balance.remaining_amount_cents,
            }),
        );
        Ok(SettleBudgetResponse {
            hold: hold.clone(),
            balance: balance.clone(),
        })
    }

    /// Release a frozen budget hold back into remaining budget.
    pub fn release_budget(
        &mut self,
        hold_id: &BudgetHoldId,
    ) -> Result<ReleaseBudgetResponse, BudgetManagerError> {
        let hold = self.budget_holds.get_mut(hold_id).ok_or_else(|| {
            log_event(
                "warn",
                "budget_release_rejected",
                json!({
                    "reason": "unknown_budget_hold",
                    "hold_id": hold_id.to_string(),
                }),
            );
            BudgetManagerError::UnknownBudgetHold
        })?;

        let balance = self
            .budget_balances
            .get_mut(&hold.budget_id)
            .ok_or_else(|| {
                log_event(
                    "warn",
                    "budget_release_rejected",
                    json!({
                        "reason": "missing_budget_balance",
                        "budget_id": hold.budget_id.to_string(),
                        "hold_id": hold.id.to_string(),
                    }),
                );
                BudgetManagerError::MissingBudgetBalance
            })?;

        hold.release()?;
        balance.frozen_amount_cents -= hold.amount_cents;
        balance.remaining_amount_cents += hold.amount_cents;

        log_event(
            "info",
            "budget_released",
            json!({
                "budget_id": hold.budget_id.to_string(),
                "hold_id": hold.id.to_string(),
                "spend_decision_id": hold.spend_decision_id.to_string(),
                "amount_cents": hold.amount_cents,
                "consumed_amount_cents": balance.consumed_amount_cents,
                "frozen_amount_cents": balance.frozen_amount_cents,
                "remaining_amount_cents": balance.remaining_amount_cents,
            }),
        );
        Ok(ReleaseBudgetResponse {
            hold: hold.clone(),
            balance: balance.clone(),
        })
    }

    /// Expire frozen holds whose authorization window has passed.
    pub fn expire_overdue_budget_holds(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<Vec<ExpireBudgetHoldResponse>, BudgetManagerError> {
        let expired_hold_ids: Vec<BudgetHoldId> = self
            .budget_holds
            .values()
            .filter(|hold| {
                matches!(hold.status, BudgetHoldStatus::Frozen) && hold.expires_at <= now
            })
            .map(|hold| hold.id.clone())
            .collect();
        let mut responses = Vec::with_capacity(expired_hold_ids.len());

        for hold_id in expired_hold_ids {
            let hold = self.budget_holds.get_mut(&hold_id).ok_or_else(|| {
                log_event(
                    "warn",
                    "budget_expire_rejected",
                    json!({
                        "reason": "unknown_budget_hold",
                        "hold_id": hold_id.to_string(),
                    }),
                );
                BudgetManagerError::UnknownBudgetHold
            })?;
            let balance = self
                .budget_balances
                .get_mut(&hold.budget_id)
                .ok_or_else(|| {
                    log_event(
                        "warn",
                        "budget_expire_rejected",
                        json!({
                            "reason": "missing_budget_balance",
                            "budget_id": hold.budget_id.to_string(),
                            "hold_id": hold.id.to_string(),
                        }),
                    );
                    BudgetManagerError::MissingBudgetBalance
                })?;

            hold.status = BudgetHoldStatus::Expired;
            hold.updated_at = now;
            balance.frozen_amount_cents -= hold.amount_cents;
            balance.remaining_amount_cents += hold.amount_cents;

            log_event(
                "info",
                "budget_hold_expired",
                json!({
                    "budget_id": hold.budget_id.to_string(),
                    "hold_id": hold.id.to_string(),
                    "spend_decision_id": hold.spend_decision_id.to_string(),
                    "amount_cents": hold.amount_cents,
                    "consumed_amount_cents": balance.consumed_amount_cents,
                    "frozen_amount_cents": balance.frozen_amount_cents,
                    "remaining_amount_cents": balance.remaining_amount_cents,
                    "expires_at": hold.expires_at.to_rfc3339(),
                }),
            );
            responses.push(ExpireBudgetHoldResponse {
                hold: hold.clone(),
                balance: balance.clone(),
            });
        }

        Ok(responses)
    }

    pub fn get_budget_by_id(&self, budget_id: &BudgetId) -> Option<BudgetWithBalance> {
        budget_with_balance(
            &self.budgets,
            &self.budget_versions,
            &self.budget_balances,
            budget_id,
        )
    }

    pub fn get_budgets_by_agent_id(&self, agent_id: &AgentId) -> Vec<BudgetWithBalance> {
        self.budget_ids_by_agent_id
            .get(agent_id)
            .map(|budget_ids| self.budgets_with_balances(budget_ids))
            .unwrap_or_default()
    }

    /// Return the immutable version history for one logical budget in ascending
    /// revision order.
    pub fn get_budget_versions_by_budget_id(&self, budget_id: &BudgetId) -> Vec<BudgetVersion> {
        let mut versions = self
            .budget_versions
            .values()
            .filter(|version| version.budget_id == *budget_id)
            .cloned()
            .collect::<Vec<_>>();
        versions.sort_by_key(|version| version.revision);
        versions
    }

    pub fn get_evaluated_budget_by_id(
        &self,
        budget_id: &BudgetId,
        now: DateTime<Utc>,
    ) -> Result<Option<EvaluatedBudget>, BudgetManagerError> {
        self.get_budget_by_id(budget_id)
            .map(|budget| budget.evaluate_at(now).map_err(Into::into))
            .transpose()
    }

    pub fn get_evaluated_budgets_by_agent_id(
        &self,
        agent_id: &AgentId,
        now: DateTime<Utc>,
    ) -> Result<Vec<EvaluatedBudget>, BudgetManagerError> {
        self.get_budgets_by_agent_id(agent_id)
            .into_iter()
            .map(|budget| budget.evaluate_at(now).map_err(Into::into))
            .collect()
    }

    pub fn available_budget_id_for_agent_at(
        &self,
        agent_id: &AgentId,
        currency: Currency,
        now: DateTime<Utc>,
    ) -> Result<Option<BudgetId>, BudgetManagerError> {
        Ok(self
            .get_evaluated_budgets_by_agent_id(agent_id, now)?
            .into_iter()
            .find(|budget| {
                budget.current.budget.currency == currency
                    && budget.availability.allows_reservation()
            })
            .map(|budget| budget.current.budget.id))
    }

    pub fn get_budget_balance(&self, budget_id: &BudgetId) -> Option<BudgetBalance> {
        self.budget_balances.get(budget_id).cloned()
    }

    pub fn get_budget_hold(&self, hold_id: &BudgetHoldId) -> Option<BudgetHold> {
        self.budget_holds.get(hold_id).cloned()
    }

    pub fn get_budget_hold_by_spend_decision(
        &self,
        spend_decision_id: &SpendDecisionId,
    ) -> Option<BudgetHold> {
        self.hold_id_by_spend_decision
            .get(spend_decision_id)
            .and_then(|hold_id| self.get_budget_hold(hold_id))
    }

    pub fn revoke_budget(
        &mut self,
        budget_id: &BudgetId,
    ) -> Result<BudgetWithBalance, BudgetManagerError> {
        self.revoke_budget_at(budget_id, Utc::now())
    }

    pub fn revoke_budget_at(
        &mut self,
        budget_id: &BudgetId,
        now: DateTime<Utc>,
    ) -> Result<BudgetWithBalance, BudgetManagerError> {
        let balance = self
            .budget_balances
            .get(budget_id)
            .ok_or(BudgetManagerError::MissingBudgetBalance)?;

        let budget = self
            .budgets
            .get_mut(budget_id)
            .ok_or(BudgetManagerError::UnknownBudget)?;
        if budget.administrative_state == BudgetAdministrativeState::Revoked {
            return Err(BudgetManagerError::BudgetAlreadyRevoked);
        }

        budget.administrative_state = BudgetAdministrativeState::Revoked;
        budget.updated_at = now;

        Ok(BudgetWithBalance {
            budget: budget.clone(),
            version: self
                .budget_versions
                .get(&budget.current_version_id)
                .cloned()
                .ok_or(BudgetManagerError::UnknownBudget)?,
            balance: balance.clone(),
        })
    }

    fn create_budget_for_period(
        &mut self,
        agent_id: AgentId,
        amount_limit_cents: i64,
        currency: Currency,
        period: TimePeriod,
        provenance: &BudgetVersionProvenance,
    ) -> Result<CreateSingleBudgetResponse, BudgetManagerError> {
        if self.has_overlapping_budget(&agent_id, currency, &period) {
            log_event(
                "warn",
                "budget_create_rejected",
                json!({
                    "reason": "overlapping_budget_period",
                    "agent_id": agent_id.to_string(),
                    "currency": currency.to_string(),
                    "starting_at": period.starting_at.to_rfc3339(),
                    "ending_before": period.ending_before.map(|value| value.to_rfc3339()),
                }),
            );
            return Err(BudgetManagerError::OverlappingBudgetPeriod);
        }

        let budget_with_balance =
            build_budget_for_period(agent_id, amount_limit_cents, currency, period, provenance)?;
        self.insert_budget(&budget_with_balance);

        log_event(
            "info",
            "budget_created",
            json!({
                "budget_id": budget_with_balance.budget.id.to_string(),
                "agent_id": budget_with_balance.budget.agent_id.to_string(),
                "budget_version_id": budget_with_balance.version.id.to_string(),
                "amount_limit_cents": budget_with_balance.version.amount_limit_cents,
                "currency": budget_with_balance.budget.currency.to_string(),
                "starting_at": budget_with_balance.budget.period.starting_at.to_rfc3339(),
                "ending_before": budget_with_balance.budget.period.ending_before.map(|value| value.to_rfc3339()),
            }),
        );
        Ok(CreateSingleBudgetResponse {
            budget: budget_with_balance.budget,
            version: budget_with_balance.version,
            balance: budget_with_balance.balance,
        })
    }

    fn insert_budget(&mut self, budget_with_balance: &BudgetWithBalance) {
        let budget = &budget_with_balance.budget;

        self.index_budget(budget);
        self.budget_versions.insert(
            budget_with_balance.version.id.clone(),
            budget_with_balance.version.clone(),
        );
        self.budget_version_id_by_revision.insert(
            (budget.id.clone(), budget_with_balance.version.revision),
            budget_with_balance.version.id.clone(),
        );
        if let Some(predecessor_id) = &budget_with_balance.version.predecessor_version_id {
            self.successor_version_id_by_predecessor.insert(
                predecessor_id.clone(),
                budget_with_balance.version.id.clone(),
            );
        }
        self.budget_balances
            .insert(budget.id.clone(), budget_with_balance.balance.clone());
        self.budgets.insert(budget.id.clone(), budget.clone());
    }

    fn index_persisted_budget_version(&mut self, version: BudgetVersion) {
        if let Some(existing_id) = self
            .budget_version_id_by_revision
            .get(&(version.budget_id.clone(), version.revision))
        {
            debug_assert_eq!(existing_id, &version.id);
        }
        if let Some(predecessor_id) = &version.predecessor_version_id {
            if let Some(existing_id) = self.successor_version_id_by_predecessor.get(predecessor_id)
            {
                debug_assert_eq!(existing_id, &version.id);
            }
            self.successor_version_id_by_predecessor
                .insert(predecessor_id.clone(), version.id.clone());
        }
        self.budget_version_id_by_revision.insert(
            (version.budget_id.clone(), version.revision),
            version.id.clone(),
        );
        self.budget_versions.insert(version.id.clone(), version);
    }

    fn budgets_with_balances(&self, budget_ids: &[BudgetId]) -> Vec<BudgetWithBalance> {
        budget_ids
            .iter()
            .filter_map(|budget_id| {
                budget_with_balance(
                    &self.budgets,
                    &self.budget_versions,
                    &self.budget_balances,
                    budget_id,
                )
            })
            .collect()
    }

    fn index_budget(&mut self, budget: &Budget) {
        self.budget_ids_by_agent_id
            .entry(budget.agent_id.clone())
            .or_default()
            .push(budget.id.clone());
    }

    fn has_overlapping_budget(
        &self,
        agent_id: &AgentId,
        currency: Currency,
        period: &TimePeriod,
    ) -> bool {
        self.budgets.values().any(|budget| {
            budget.administrative_state != BudgetAdministrativeState::Revoked
                && budget.currency == currency
                && budget.agent_id == *agent_id
                && periods_overlap(&budget.period, period)
        })
    }
}

fn log_budget_reservation_rejected(request: &ReserveBudgetRequest, reason: &str) {
    log_event(
        "warn",
        "budget_reservation_rejected",
        json!({
            "reason": reason,
            "budget_id": request.budget_id.to_string(),
            "spend_decision_id": request.spend_decision_id.to_string(),
            "amount_cents": request.amount_cents,
            "currency": request.currency.to_string(),
            "expires_at": request.expires_at.to_rfc3339(),
        }),
    );
}

fn periods_overlap(left: &TimePeriod, right: &TimePeriod) -> bool {
    let left_starts_before_right_ends = right
        .ending_before
        .is_none_or(|right_end| left.starting_at < right_end);
    let right_starts_before_left_ends = left
        .ending_before
        .is_none_or(|left_end| right.starting_at < left_end);

    left_starts_before_right_ends && right_starts_before_left_ends
}

fn build_budget_for_period(
    agent_id: AgentId,
    amount_limit_cents: i64,
    currency: Currency,
    period: TimePeriod,
    provenance: &BudgetVersionProvenance,
) -> Result<BudgetWithBalance, BudgetManagerError> {
    if provenance.actor.trim().is_empty() || provenance.source.trim().is_empty() {
        return Err(BudgetManagerError::MissingBudgetVersionProvenance);
    }
    let budget = Budget::new(BudgetId::new(), agent_id, currency, period);
    let version = BudgetVersion::initial(
        &budget,
        amount_limit_cents,
        provenance.actor.clone(),
        provenance.source.clone(),
        provenance.reason.clone(),
    )?;
    let balance = BudgetBalance {
        budget_id: budget.id.clone(),
        consumed_amount_cents: 0,
        frozen_amount_cents: 0,
        remaining_amount_cents: version.amount_limit_cents,
    };

    Ok(BudgetWithBalance {
        budget,
        version,
        balance,
    })
}

fn budget_with_balance(
    budgets: &HashMap<BudgetId, Budget>,
    budget_versions: &HashMap<BudgetVersionId, BudgetVersion>,
    budget_balances: &HashMap<BudgetId, BudgetBalance>,
    budget_id: &BudgetId,
) -> Option<BudgetWithBalance> {
    let budget = budgets.get(budget_id)?.clone();
    let version = budget_versions.get(&budget.current_version_id)?.clone();
    Some(BudgetWithBalance {
        budget,
        version,
        balance: budget_balances.get(budget_id)?.clone(),
    })
}

fn next_period_boundary(
    starting_at: DateTime<Utc>,
    recurrence: BudgetRecurrence,
) -> Result<DateTime<Utc>, BudgetManagerError> {
    match recurrence {
        BudgetRecurrence::Daily => starting_at
            .checked_add_signed(Duration::days(1))
            .ok_or(BudgetManagerError::InvalidRecurrenceBoundary),
        BudgetRecurrence::Monthly => starting_at
            .checked_add_months(Months::new(1))
            .ok_or(BudgetManagerError::InvalidRecurrenceBoundary),
        BudgetRecurrence::Yearly => starting_at
            .checked_add_months(Months::new(12))
            .ok_or(BudgetManagerError::InvalidRecurrenceBoundary),
    }
}

impl Default for BudgetManager {
    fn default() -> Self {
        Self::new()
    }
}

fn invalid_persisted_budget_state(message: impl Into<String>) -> BudgetManagerError {
    BudgetManagerError::InvalidPersistedState(message.into())
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::budget::BudgetAvailability;

    fn timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap()
    }

    fn active_period() -> TimePeriod {
        TimePeriod::new(
            Utc::now() - Duration::hours(1),
            Some(Utc::now() + Duration::hours(1)),
        )
        .unwrap()
    }

    fn period(
        start_year: i32,
        start_month: u32,
        start_day: u32,
        end_year: i32,
        end_month: u32,
        end_day: u32,
    ) -> TimePeriod {
        TimePeriod::new(
            Utc.with_ymd_and_hms(start_year, start_month, start_day, 0, 0, 0)
                .unwrap(),
            Some(
                Utc.with_ymd_and_hms(end_year, end_month, end_day, 0, 0, 0)
                    .unwrap(),
            ),
        )
        .unwrap()
    }

    fn create_agent_budget(manager: &mut BudgetManager, amount_cents: i64) -> BudgetWithBalance {
        let response = manager
            .create_single_budget(CreateSingleBudgetRequest {
                agent_id: AgentId::new(),
                amount_limit_cents: amount_cents,
                currency: Currency::Usd,
                period: active_period(),
            })
            .expect("budget should be created");

        BudgetWithBalance {
            budget: response.budget,
            version: response.version,
            balance: response.balance,
        }
    }

    #[test]
    fn create_single_budget_initializes_balance() {
        let mut manager = BudgetManager::new();

        let created = create_agent_budget(&mut manager, 10_000);

        assert_eq!(created.budget.current_version_id, created.version.id);
        assert_eq!(created.version.budget_id, created.budget.id);
        assert_eq!(created.version.revision, 1);
        assert!(created.version.predecessor_version_id.is_none());
        assert_eq!(created.version.amount_limit_cents, 10_000);
        assert!(!created.version.actor.is_empty());
        assert!(!created.version.source.is_empty());
        assert!(created.version.request_fingerprint.starts_with("sha256:"));
        assert_eq!(created.balance.budget_id, created.budget.id);
        assert_eq!(created.balance.consumed_amount_cents, 0);
        assert_eq!(created.balance.frozen_amount_cents, 0);
        assert_eq!(created.balance.remaining_amount_cents, 10_000);
        assert!(manager.get_budget_by_id(&created.budget.id).is_some());
    }

    #[test]
    fn hydration_selects_latest_version_and_attributes_new_hold_to_it() {
        let mut original_manager = BudgetManager::new();
        let created = create_agent_budget(&mut original_manager, 10_000);
        let second_version = BudgetVersion {
            id: BudgetVersionId::new(),
            budget_id: created.budget.id.clone(),
            revision: 2,
            predecessor_version_id: Some(created.version.id.clone()),
            amount_limit_cents: 20_000,
            effective_at: Utc::now(),
            actor: "test:budget-owner".to_string(),
            source: "manager-hydration-test".to_string(),
            reason: Some("test current-version selection".to_string()),
            request_fingerprint: "sha256:test-revision-2".to_string(),
            created_at: Utc::now(),
        };
        let mut logical_budget = created.budget;
        logical_budget.current_version_id = second_version.id.clone();
        let balance = BudgetBalance {
            budget_id: logical_budget.id.clone(),
            consumed_amount_cents: 0,
            frozen_amount_cents: 0,
            remaining_amount_cents: 20_000,
        };

        let mut hydrated = BudgetManager::from_records(
            vec![logical_budget.clone()],
            vec![created.version, second_version.clone()],
            vec![balance],
            vec![],
        )
        .expect("version chain should hydrate");
        let resolved = hydrated
            .get_budget_by_id(&logical_budget.id)
            .expect("logical budget should resolve");
        assert_eq!(resolved.version.id, second_version.id);

        let reservation = hydrated
            .reserve_budget(ReserveBudgetRequest {
                budget_id: logical_budget.id,
                spend_decision_id: SpendDecisionId::new(),
                amount_cents: 15_000,
                currency: Currency::Usd,
                expires_at: Utc::now() + Duration::minutes(5),
            })
            .expect("current version limit should authorize the hold");
        assert_eq!(reservation.hold.budget_version_id, second_version.id);
    }

    #[test]
    fn budget_version_history_is_sorted_and_scoped_to_one_logical_budget() {
        let mut original_manager = BudgetManager::new();
        let created = create_agent_budget(&mut original_manager, 10_000);
        let unrelated = create_agent_budget(&mut original_manager, 5_000);
        let second_version = BudgetVersion {
            id: BudgetVersionId::new(),
            budget_id: created.budget.id.clone(),
            revision: 2,
            predecessor_version_id: Some(created.version.id.clone()),
            amount_limit_cents: 20_000,
            effective_at: Utc::now(),
            actor: "test:budget-owner".to_string(),
            source: "manager-history-test".to_string(),
            reason: Some("test ordered history".to_string()),
            request_fingerprint: "sha256:test-history-revision-2".to_string(),
            created_at: Utc::now(),
        };
        let mut logical_budget = created.budget;
        logical_budget.current_version_id = second_version.id.clone();
        let logical_budget_id = logical_budget.id.clone();

        let hydrated = BudgetManager::from_records(
            vec![unrelated.budget, logical_budget],
            vec![
                second_version.clone(),
                unrelated.version,
                created.version.clone(),
            ],
            vec![
                unrelated.balance,
                BudgetBalance {
                    budget_id: logical_budget_id.clone(),
                    consumed_amount_cents: 0,
                    frozen_amount_cents: 0,
                    remaining_amount_cents: 20_000,
                },
            ],
            vec![],
        )
        .expect("version graph should hydrate from unordered records");

        let versions = hydrated.get_budget_versions_by_budget_id(&logical_budget_id);
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].id, created.version.id);
        assert_eq!(versions[0].revision, 1);
        assert_eq!(versions[1].id, second_version.id);
        assert_eq!(versions[1].revision, 2);
        assert!(versions
            .iter()
            .all(|version| version.budget_id == logical_budget_id));
        assert!(hydrated
            .get_budget_versions_by_budget_id(&BudgetId::new())
            .is_empty());
    }

    #[test]
    fn hydration_rejects_balance_that_does_not_derive_from_current_version() {
        let mut manager = BudgetManager::new();
        let created = create_agent_budget(&mut manager, 10_000);
        let invalid_balance = BudgetBalance {
            budget_id: created.budget.id.clone(),
            consumed_amount_cents: 1_000,
            frozen_amount_cents: 0,
            remaining_amount_cents: 10_000,
        };

        let error = BudgetManager::from_records(
            vec![created.budget],
            vec![created.version],
            vec![invalid_balance],
            vec![],
        )
        .expect_err("cached remaining must be exhaustively validated");

        assert!(matches!(
            error,
            BudgetManagerError::InvalidPersistedState(_)
        ));
    }

    #[test]
    fn hydration_rejects_non_revoked_overlap_independent_of_input_order() {
        let agent_id = AgentId::new();
        let start = timestamp();
        let provenance = BudgetVersionProvenance::default();
        let first = build_budget_for_period(
            agent_id.clone(),
            1_000,
            Currency::Usd,
            TimePeriod::new(start, Some(start + Duration::hours(2))).unwrap(),
            &provenance,
        )
        .unwrap();
        let second = build_budget_for_period(
            agent_id.clone(),
            1_000,
            Currency::Usd,
            TimePeriod::new(start + Duration::hours(1), Some(start + Duration::hours(3))).unwrap(),
            &provenance,
        )
        .unwrap();
        let hydrate = |snapshots: Vec<BudgetWithBalance>| {
            BudgetManager::from_records(
                snapshots
                    .iter()
                    .map(|snapshot| snapshot.budget.clone())
                    .collect(),
                snapshots
                    .iter()
                    .map(|snapshot| snapshot.version.clone())
                    .collect(),
                snapshots
                    .iter()
                    .map(|snapshot| snapshot.balance.clone())
                    .collect(),
                vec![],
            )
        };

        for snapshots in [
            vec![first.clone(), second.clone()],
            vec![second.clone(), first.clone()],
        ] {
            let error = hydrate(snapshots).expect_err("overlap must fail in either input order");
            let message = error.to_string();
            assert!(message.contains(&first.budget.id.to_string()));
            assert!(message.contains(&second.budget.id.to_string()));
            assert!(message.contains("overlap"));
        }
    }

    #[test]
    fn hydration_allows_adjacent_and_revoked_overlapping_budgets() {
        let agent_id = AgentId::new();
        let start = timestamp();
        let boundary = start + Duration::hours(1);
        let provenance = BudgetVersionProvenance::default();
        let first = build_budget_for_period(
            agent_id.clone(),
            1_000,
            Currency::Usd,
            TimePeriod::new(start, Some(boundary)).unwrap(),
            &provenance,
        )
        .unwrap();
        let second = build_budget_for_period(
            agent_id.clone(),
            1_000,
            Currency::Usd,
            TimePeriod::new(boundary, Some(boundary + Duration::hours(1))).unwrap(),
            &provenance,
        )
        .unwrap();
        let hydrate = |snapshots: &[BudgetWithBalance]| {
            BudgetManager::from_records(
                snapshots
                    .iter()
                    .map(|snapshot| snapshot.budget.clone())
                    .collect(),
                snapshots
                    .iter()
                    .map(|snapshot| snapshot.version.clone())
                    .collect(),
                snapshots
                    .iter()
                    .map(|snapshot| snapshot.balance.clone())
                    .collect(),
                vec![],
            )
        };

        hydrate(&[first.clone(), second.clone()]).expect("half-open adjacent budgets must hydrate");

        let mut revoked_overlap = build_budget_for_period(
            agent_id,
            1_000,
            Currency::Usd,
            first.budget.period.clone(),
            &provenance,
        )
        .unwrap();
        revoked_overlap.budget.administrative_state = BudgetAdministrativeState::Revoked;
        hydrate(&[first, revoked_overlap])
            .expect("an administratively revoked overlap must hydrate");
    }

    #[test]
    fn revoke_budget_marks_budget_inactive() {
        let mut manager = BudgetManager::new();
        let created = create_agent_budget(&mut manager, 10_000);

        let revoked = manager
            .revoke_budget(&created.budget.id)
            .expect("active budget without holds should revoke");

        assert_eq!(
            revoked.budget.administrative_state,
            BudgetAdministrativeState::Revoked
        );
        assert_eq!(
            manager
                .get_evaluated_budget_by_id(&created.budget.id, Utc::now())
                .unwrap()
                .expect("budget should remain available")
                .availability,
            BudgetAvailability::Revoked
        );
    }

    #[test]
    fn revoke_budget_allows_outstanding_holds_to_finalize() {
        let mut manager = BudgetManager::new();
        let created = create_agent_budget(&mut manager, 10_000);
        let reservation = manager
            .reserve_budget(ReserveBudgetRequest {
                budget_id: created.budget.id.clone(),
                spend_decision_id: SpendDecisionId::new(),
                amount_cents: 1_000,
                currency: Currency::Usd,
                expires_at: Utc::now() + Duration::minutes(5),
            })
            .expect("budget should reserve");

        let revoked = manager
            .revoke_budget(&created.budget.id)
            .expect("budget with frozen holds may revoke");
        assert_eq!(
            revoked.budget.administrative_state,
            BudgetAdministrativeState::Revoked
        );

        let settled = manager
            .settle_budget(&reservation.hold.id)
            .expect("the outstanding hold should still settle");
        assert_eq!(settled.balance.consumed_amount_cents, 1_000);
        assert_eq!(
            manager
                .get_evaluated_budget_by_id(&created.budget.id, Utc::now())
                .unwrap()
                .unwrap()
                .availability,
            BudgetAvailability::Revoked
        );
    }

    #[test]
    fn revoked_budget_does_not_block_replacement_period() {
        let mut manager = BudgetManager::new();
        let agent_id = AgentId::new();
        let created = manager
            .create_single_budget(CreateSingleBudgetRequest {
                agent_id: agent_id.clone(),
                amount_limit_cents: 10_000,
                currency: Currency::Usd,
                period: period(2026, 6, 1, 2026, 7, 1),
            })
            .expect("first budget should be created");

        manager
            .revoke_budget(&created.budget.id)
            .expect("budget should revoke");
        manager
            .create_single_budget(CreateSingleBudgetRequest {
                agent_id,
                amount_limit_cents: 20_000,
                currency: Currency::Usd,
                period: period(2026, 6, 15, 2026, 7, 1),
            })
            .expect("revoked budget should not block overlapping replacement");
    }

    #[test]
    fn create_budget_series_creates_adjacent_periods() {
        let mut manager = BudgetManager::new();

        let response = manager
            .create_budget_series(CreateBudgetSeriesRequest {
                agent_id: AgentId::new(),
                amount_limit_cents: 25_000,
                currency: Currency::Usd,
                starting_at: timestamp(),
                recurrence: BudgetRecurrence::Monthly,
                period_count: 2,
            })
            .expect("budget series should be created");

        assert_eq!(response.budgets.len(), 2);
        assert_eq!(
            response.budgets[0].budget.period.ending_before,
            Some(response.budgets[1].budget.period.starting_at)
        );
        assert_eq!(response.budgets[0].balance.remaining_amount_cents, 25_000);
        assert_eq!(response.budgets[1].balance.remaining_amount_cents, 25_000);
    }

    #[test]
    fn reservation_uses_effective_availability_at_exact_boundaries() {
        let start = timestamp();
        let end = start + Duration::hours(1);

        let mut scheduled_manager = BudgetManager::new();
        let scheduled = scheduled_manager
            .create_single_budget(CreateSingleBudgetRequest {
                agent_id: AgentId::new(),
                amount_limit_cents: 1_000,
                currency: Currency::Usd,
                period: TimePeriod::new(start, Some(end)).unwrap(),
            })
            .unwrap();
        let request = |budget_id| ReserveBudgetRequest {
            budget_id,
            spend_decision_id: SpendDecisionId::new(),
            amount_cents: 100,
            currency: Currency::Usd,
            expires_at: end + Duration::hours(1),
        };
        assert!(matches!(
            scheduled_manager.reserve_budget_at(
                request(scheduled.budget.id.clone()),
                start - Duration::nanoseconds(1),
            ),
            Err(BudgetManagerError::BudgetUnavailable(
                BudgetAvailability::Scheduled
            ))
        ));
        scheduled_manager
            .reserve_budget_at(request(scheduled.budget.id.clone()), start)
            .expect("the half-open period starts exactly at starting_at");

        let expired = scheduled_manager
            .reserve_budget_at(request(scheduled.budget.id.clone()), end)
            .expect_err("the half-open period ends exactly at ending_before");
        assert!(matches!(
            expired,
            BudgetManagerError::BudgetUnavailable(BudgetAvailability::Expired)
        ));

        let remaining = scheduled_manager
            .get_budget_balance(&scheduled.budget.id)
            .unwrap()
            .remaining_amount_cents;
        scheduled_manager
            .reserve_budget_at(
                ReserveBudgetRequest {
                    amount_cents: remaining,
                    ..request(scheduled.budget.id.clone())
                },
                start + Duration::minutes(1),
            )
            .unwrap();
        assert!(matches!(
            scheduled_manager.reserve_budget_at(
                request(scheduled.budget.id.clone()),
                start + Duration::minutes(2),
            ),
            Err(BudgetManagerError::BudgetUnavailable(
                BudgetAvailability::Exhausted
            ))
        ));

        scheduled_manager
            .revoke_budget_at(&scheduled.budget.id, start + Duration::minutes(3))
            .unwrap();
        assert!(matches!(
            scheduled_manager
                .reserve_budget_at(request(scheduled.budget.id), start + Duration::minutes(4),),
            Err(BudgetManagerError::BudgetUnavailable(
                BudgetAvailability::Revoked
            ))
        ));
    }

    #[test]
    fn adjacent_period_selection_switches_exactly_at_the_shared_boundary() {
        let mut manager = BudgetManager::new();
        let agent_id = AgentId::new();
        let series = manager
            .create_budget_series(CreateBudgetSeriesRequest {
                agent_id: agent_id.clone(),
                amount_limit_cents: 1_000,
                currency: Currency::Usd,
                starting_at: timestamp(),
                recurrence: BudgetRecurrence::Daily,
                period_count: 2,
            })
            .unwrap();
        let boundary = series.budgets[1].budget.period.starting_at;

        assert_eq!(
            manager
                .available_budget_id_for_agent_at(
                    &agent_id,
                    Currency::Usd,
                    boundary - Duration::nanoseconds(1),
                )
                .unwrap(),
            Some(series.budgets[0].budget.id.clone())
        );
        assert_eq!(
            manager
                .available_budget_id_for_agent_at(&agent_id, Currency::Usd, boundary)
                .unwrap(),
            Some(series.budgets[1].budget.id.clone())
        );
    }

    #[test]
    fn hold_release_and_expiry_after_budget_end_do_not_reactivate_it() {
        let start = timestamp();
        let end = start + Duration::hours(1);
        let mut manager = BudgetManager::new();
        let created = manager
            .create_single_budget(CreateSingleBudgetRequest {
                agent_id: AgentId::new(),
                amount_limit_cents: 1_000,
                currency: Currency::Usd,
                period: TimePeriod::new(start, Some(end)).unwrap(),
            })
            .unwrap();
        let original_updated_at = created.budget.updated_at;
        let first = manager
            .reserve_budget_at(
                ReserveBudgetRequest {
                    budget_id: created.budget.id.clone(),
                    spend_decision_id: SpendDecisionId::new(),
                    amount_cents: 400,
                    currency: Currency::Usd,
                    expires_at: end + Duration::minutes(1),
                },
                start,
            )
            .unwrap();
        manager.release_budget(&first.hold.id).unwrap();
        assert_eq!(
            manager
                .get_evaluated_budget_by_id(&created.budget.id, end)
                .unwrap()
                .unwrap()
                .availability,
            BudgetAvailability::Expired
        );

        let second = manager
            .reserve_budget_at(
                ReserveBudgetRequest {
                    budget_id: created.budget.id.clone(),
                    spend_decision_id: SpendDecisionId::new(),
                    amount_cents: 400,
                    currency: Currency::Usd,
                    expires_at: end,
                },
                start + Duration::minutes(1),
            )
            .unwrap();
        let expired = manager.expire_overdue_budget_holds(end).unwrap();
        assert_eq!(expired[0].hold.id, second.hold.id);
        let snapshot = manager.get_budget_by_id(&created.budget.id).unwrap();
        assert_eq!(snapshot.budget.updated_at, original_updated_at);
        assert_eq!(
            snapshot.availability_at(end).unwrap(),
            BudgetAvailability::Expired
        );
    }

    #[test]
    fn create_budget_series_rejects_overlap_without_partial_creation() {
        let mut manager = BudgetManager::new();
        let agent_id = AgentId::new();

        manager
            .create_single_budget(CreateSingleBudgetRequest {
                agent_id: agent_id.clone(),
                amount_limit_cents: 10_000,
                currency: Currency::Usd,
                period: period(2026, 7, 15, 2026, 8, 15),
            })
            .expect("existing budget should be created");

        let error = manager
            .create_budget_series(CreateBudgetSeriesRequest {
                agent_id: agent_id.clone(),
                amount_limit_cents: 25_000,
                currency: Currency::Usd,
                starting_at: timestamp(),
                recurrence: BudgetRecurrence::Monthly,
                period_count: 2,
            })
            .expect_err("series should be rejected before creating any budget");

        assert!(matches!(error, BudgetManagerError::OverlappingBudgetPeriod));
        assert_eq!(manager.get_budgets_by_agent_id(&agent_id).len(), 1);
    }

    #[test]
    fn create_budget_series_rejects_invalid_budget_without_partial_creation() {
        let mut manager = BudgetManager::new();
        let agent_id = AgentId::new();

        let error = manager
            .create_budget_series(CreateBudgetSeriesRequest {
                agent_id: agent_id.clone(),
                amount_limit_cents: 0,
                currency: Currency::Usd,
                starting_at: timestamp(),
                recurrence: BudgetRecurrence::Monthly,
                period_count: 2,
            })
            .expect_err("invalid series should be rejected before creating any budget");

        assert!(matches!(error, BudgetManagerError::InvalidBudget(_)));
        assert!(manager.get_budgets_by_agent_id(&agent_id).is_empty());
    }

    #[test]
    fn create_single_budget_rejects_overlap_for_same_agent_and_currency() {
        let mut manager = BudgetManager::new();
        let agent_id = AgentId::new();

        manager
            .create_single_budget(CreateSingleBudgetRequest {
                agent_id: agent_id.clone(),
                amount_limit_cents: 10_000,
                currency: Currency::Usd,
                period: period(2026, 6, 1, 2026, 7, 1),
            })
            .expect("first budget should be created");

        let error = manager
            .create_single_budget(CreateSingleBudgetRequest {
                agent_id,
                amount_limit_cents: 10_000,
                currency: Currency::Usd,
                period: period(2026, 6, 15, 2026, 7, 15),
            })
            .expect_err("overlapping budget should be rejected");

        assert!(matches!(error, BudgetManagerError::OverlappingBudgetPeriod));
    }

    #[test]
    fn create_single_budget_allows_adjacent_half_open_periods() {
        let mut manager = BudgetManager::new();
        let agent_id = AgentId::new();

        manager
            .create_single_budget(CreateSingleBudgetRequest {
                agent_id: agent_id.clone(),
                amount_limit_cents: 10_000,
                currency: Currency::Usd,
                period: period(2026, 6, 1, 2026, 7, 1),
            })
            .expect("first budget should be created");

        manager
            .create_single_budget(CreateSingleBudgetRequest {
                agent_id,
                amount_limit_cents: 10_000,
                currency: Currency::Usd,
                period: period(2026, 7, 1, 2026, 8, 1),
            })
            .expect("adjacent budget should be created");
    }

    #[test]
    fn create_single_budget_rejects_overlap_with_open_ended_period() {
        let mut manager = BudgetManager::new();
        let agent_id = AgentId::new();
        let open_ended_period =
            TimePeriod::new(Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap(), None).unwrap();

        manager
            .create_single_budget(CreateSingleBudgetRequest {
                agent_id: agent_id.clone(),
                amount_limit_cents: 10_000,
                currency: Currency::Usd,
                period: open_ended_period,
            })
            .expect("open-ended budget should be created");

        let error = manager
            .create_single_budget(CreateSingleBudgetRequest {
                agent_id,
                amount_limit_cents: 10_000,
                currency: Currency::Usd,
                period: period(2026, 7, 1, 2026, 8, 1),
            })
            .expect_err("overlap with open-ended budget should be rejected");

        assert!(matches!(error, BudgetManagerError::OverlappingBudgetPeriod));
    }

    #[test]
    fn reserve_budget_freezes_amount_and_reduces_remaining() {
        let mut manager = BudgetManager::new();
        let created = create_agent_budget(&mut manager, 10_000);

        let response = manager
            .reserve_budget(ReserveBudgetRequest {
                budget_id: created.budget.id,
                spend_decision_id: SpendDecisionId::new(),
                amount_cents: 3_000,
                currency: Currency::Usd,
                expires_at: Utc::now() + Duration::minutes(5),
            })
            .expect("budget should be reserved");

        assert!(matches!(response.hold.status, BudgetHoldStatus::Frozen));
        assert_eq!(response.hold.budget_version_id, created.version.id);
        assert_eq!(response.balance.frozen_amount_cents, 3_000);
        assert_eq!(response.balance.remaining_amount_cents, 7_000);
        assert_eq!(response.balance.consumed_amount_cents, 0);
    }

    #[test]
    fn claimed_budget_hold_uses_claim_lease_and_does_not_auto_release() {
        let mut manager = BudgetManager::new();
        let created = create_agent_budget(&mut manager, 10_000);
        let authorization_expires_at = Utc::now() + Duration::minutes(5);
        let reservation = manager
            .reserve_budget(ReserveBudgetRequest {
                budget_id: created.budget.id,
                spend_decision_id: SpendDecisionId::new(),
                amount_cents: 3_000,
                currency: Currency::Usd,
                expires_at: authorization_expires_at,
            })
            .expect("budget should reserve");
        let claim_id = SpendExecutorClaimId::new();
        let claim_expires_at = Utc::now() + Duration::minutes(30);

        let claimed = manager
            .claim_budget(&reservation.hold.id, claim_id.clone(), claim_expires_at)
            .expect("hold should enter claimed state");
        assert!(matches!(claimed.hold.status, BudgetHoldStatus::Claimed));
        assert_eq!(claimed.hold.executor_claim_id, Some(claim_id));
        assert_eq!(claimed.hold.expires_at, claim_expires_at);

        let expired = manager
            .expire_overdue_budget_holds(authorization_expires_at + Duration::seconds(1))
            .expect("expiry reconciliation should succeed");
        assert!(expired.is_empty());
        assert!(matches!(
            manager
                .get_budget_hold(&reservation.hold.id)
                .expect("claimed hold should remain"),
            BudgetHold {
                status: BudgetHoldStatus::Claimed,
                ..
            }
        ));
    }

    #[test]
    fn reserve_budget_rejects_duplicate_spend_decision() {
        let mut manager = BudgetManager::new();
        let created = create_agent_budget(&mut manager, 10_000);
        let spend_decision_id = SpendDecisionId::new();

        manager
            .reserve_budget(ReserveBudgetRequest {
                budget_id: created.budget.id.clone(),
                spend_decision_id: spend_decision_id.clone(),
                amount_cents: 3_000,
                currency: Currency::Usd,
                expires_at: Utc::now() + Duration::minutes(5),
            })
            .expect("first reservation should succeed");

        let error = manager
            .reserve_budget(ReserveBudgetRequest {
                budget_id: created.budget.id,
                spend_decision_id,
                amount_cents: 3_000,
                currency: Currency::Usd,
                expires_at: Utc::now() + Duration::minutes(5),
            })
            .expect_err("duplicate reservation should fail");

        assert!(matches!(
            error,
            BudgetManagerError::DuplicateSpendDecisionHold
        ));
    }

    #[test]
    fn reserve_budget_rejects_overspend() {
        let mut manager = BudgetManager::new();
        let created = create_agent_budget(&mut manager, 10_000);

        let error = manager
            .reserve_budget(ReserveBudgetRequest {
                budget_id: created.budget.id,
                spend_decision_id: SpendDecisionId::new(),
                amount_cents: 10_001,
                currency: Currency::Usd,
                expires_at: Utc::now() + Duration::minutes(5),
            })
            .expect_err("overspend should fail");

        assert!(matches!(
            error,
            BudgetManagerError::InsufficientRemainingBudget
        ));
    }

    #[test]
    fn settle_budget_moves_frozen_amount_to_consumed() {
        let mut manager = BudgetManager::new();
        let created = create_agent_budget(&mut manager, 10_000);
        let reservation = manager
            .reserve_budget(ReserveBudgetRequest {
                budget_id: created.budget.id,
                spend_decision_id: SpendDecisionId::new(),
                amount_cents: 3_000,
                currency: Currency::Usd,
                expires_at: Utc::now() + Duration::minutes(5),
            })
            .expect("budget should be reserved");

        let response = manager
            .settle_budget(&reservation.hold.id)
            .expect("hold should settle");

        assert!(matches!(response.hold.status, BudgetHoldStatus::Settled));
        assert_eq!(response.balance.frozen_amount_cents, 0);
        assert_eq!(response.balance.consumed_amount_cents, 3_000);
        assert_eq!(response.balance.remaining_amount_cents, 7_000);
    }

    #[test]
    fn fully_reserved_budget_is_immediately_and_still_exhausted_after_settlement() {
        let mut manager = BudgetManager::new();
        let created = create_agent_budget(&mut manager, 1_000);
        let budget_id = created.budget.id.clone();
        let reservation = manager
            .reserve_budget(ReserveBudgetRequest {
                budget_id: budget_id.clone(),
                spend_decision_id: SpendDecisionId::new(),
                amount_cents: 1_000,
                currency: Currency::Usd,
                expires_at: Utc::now() + Duration::minutes(5),
            })
            .expect("budget should be reserved");

        let reserved = manager
            .get_budget_by_id(&budget_id)
            .expect("budget should exist");
        assert_eq!(
            reserved.availability_at(Utc::now()).unwrap(),
            BudgetAvailability::Exhausted
        );
        assert_eq!(reserved.balance.remaining_amount_cents, 0);
        assert_eq!(reserved.balance.frozen_amount_cents, 1_000);

        manager
            .settle_budget(&reservation.hold.id)
            .expect("hold should settle");
        let settled = manager
            .get_budget_by_id(&budget_id)
            .expect("budget should exist");
        assert_eq!(
            settled.availability_at(Utc::now()).unwrap(),
            BudgetAvailability::Exhausted
        );
        assert_eq!(settled.balance.remaining_amount_cents, 0);
        assert_eq!(settled.balance.frozen_amount_cents, 0);
    }

    #[test]
    fn released_full_reservation_restores_active_budget() {
        let mut manager = BudgetManager::new();
        let created = create_agent_budget(&mut manager, 1_000);
        let budget_id = created.budget.id.clone();
        let reservation = manager
            .reserve_budget(ReserveBudgetRequest {
                budget_id: budget_id.clone(),
                spend_decision_id: SpendDecisionId::new(),
                amount_cents: 1_000,
                currency: Currency::Usd,
                expires_at: Utc::now() + Duration::minutes(5),
            })
            .expect("budget should be reserved");

        manager
            .release_budget(&reservation.hold.id)
            .expect("hold should release");
        let released = manager
            .get_budget_by_id(&budget_id)
            .expect("budget should exist");
        assert_eq!(
            released.availability_at(Utc::now()).unwrap(),
            BudgetAvailability::Active
        );
        assert_eq!(released.balance.remaining_amount_cents, 1_000);
        assert_eq!(released.balance.frozen_amount_cents, 0);
    }

    #[test]
    fn exhausted_budget_blocks_new_overlapping_budget() {
        let mut manager = BudgetManager::new();
        let agent_id = AgentId::new();
        let created = manager
            .create_single_budget(CreateSingleBudgetRequest {
                agent_id: agent_id.clone(),
                amount_limit_cents: 1_000,
                currency: Currency::Usd,
                period: active_period(),
            })
            .expect("budget should be created");
        let reservation = manager
            .reserve_budget(ReserveBudgetRequest {
                budget_id: created.budget.id.clone(),
                spend_decision_id: SpendDecisionId::new(),
                amount_cents: 1_000,
                currency: Currency::Usd,
                expires_at: Utc::now() + Duration::minutes(5),
            })
            .expect("budget should be reserved");
        manager
            .settle_budget(&reservation.hold.id)
            .expect("hold should settle");

        let error = manager
            .create_single_budget(CreateSingleBudgetRequest {
                agent_id,
                amount_limit_cents: 2_000,
                currency: Currency::Usd,
                period: active_period(),
            })
            .expect_err("exhausted budget may be increased and must block overlap");

        assert!(matches!(error, BudgetManagerError::OverlappingBudgetPeriod));
    }

    #[test]
    fn settle_budget_rejects_expired_hold_without_consuming_balance() {
        let mut manager = BudgetManager::new();
        let created = create_agent_budget(&mut manager, 10_000);
        let reservation = manager
            .reserve_budget(ReserveBudgetRequest {
                budget_id: created.budget.id.clone(),
                spend_decision_id: SpendDecisionId::new(),
                amount_cents: 3_000,
                currency: Currency::Usd,
                expires_at: Utc::now() - Duration::minutes(1),
            })
            .expect("budget should be reserved");

        let error = manager
            .settle_budget(&reservation.hold.id)
            .expect_err("expired hold should not settle");
        let balance = manager
            .get_budget_balance(&created.budget.id)
            .expect("balance should exist");
        let hold = manager
            .budget_holds
            .get(&reservation.hold.id)
            .expect("hold should exist");

        assert!(matches!(error, BudgetManagerError::ExpiredBudgetHold));
        assert!(matches!(hold.status, BudgetHoldStatus::Frozen));
        assert_eq!(balance.frozen_amount_cents, 3_000);
        assert_eq!(balance.consumed_amount_cents, 0);
        assert_eq!(balance.remaining_amount_cents, 7_000);
    }

    #[test]
    fn release_budget_moves_frozen_amount_to_remaining() {
        let mut manager = BudgetManager::new();
        let created = create_agent_budget(&mut manager, 10_000);
        let reservation = manager
            .reserve_budget(ReserveBudgetRequest {
                budget_id: created.budget.id,
                spend_decision_id: SpendDecisionId::new(),
                amount_cents: 3_000,
                currency: Currency::Usd,
                expires_at: Utc::now() + Duration::minutes(5),
            })
            .expect("budget should be reserved");

        let response = manager
            .release_budget(&reservation.hold.id)
            .expect("hold should release");

        assert!(matches!(response.hold.status, BudgetHoldStatus::Released));
        assert_eq!(response.balance.frozen_amount_cents, 0);
        assert_eq!(response.balance.consumed_amount_cents, 0);
        assert_eq!(response.balance.remaining_amount_cents, 10_000);
    }

    #[test]
    fn expire_overdue_budget_holds_returns_frozen_amount_to_remaining() {
        let mut manager = BudgetManager::new();
        let created = create_agent_budget(&mut manager, 10_000);
        let reservation = manager
            .reserve_budget(ReserveBudgetRequest {
                budget_id: created.budget.id.clone(),
                spend_decision_id: SpendDecisionId::new(),
                amount_cents: 3_000,
                currency: Currency::Usd,
                expires_at: Utc::now() + Duration::minutes(5),
            })
            .expect("budget should be reserved");

        let expired = manager
            .expire_overdue_budget_holds(reservation.hold.expires_at + Duration::seconds(1))
            .expect("overdue hold should expire");
        let balance = manager
            .get_budget_balance(&created.budget.id)
            .expect("balance should exist");

        assert_eq!(expired.len(), 1);
        assert!(matches!(expired[0].hold.status, BudgetHoldStatus::Expired));
        assert_eq!(expired[0].balance.frozen_amount_cents, 0);
        assert_eq!(expired[0].balance.remaining_amount_cents, 10_000);
        assert_eq!(balance.frozen_amount_cents, 0);
        assert_eq!(balance.consumed_amount_cents, 0);
        assert_eq!(balance.remaining_amount_cents, 10_000);
    }
}
