use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use hubu_common::ids::{
    AgentId, BudgetHoldId, BudgetId, BudgetVersionId, SpendDecisionId, SpendExecutorClaimId,
};
use hubu_common::money::Currency;
use hubu_common::time::TimePeriod;
use serde_json::json;

use crate::budget::dto::{
    BudgetWithBalance, CreateSingleBudgetRequest, CreateSingleBudgetResponse, EvaluatedBudget,
    ExpireBudgetHoldResponse, ReleaseBudgetResponse, ReserveBudgetRequest, ReserveBudgetResponse,
    SettleBudgetResponse,
};
use crate::budget::error::BudgetManagerError;
use crate::budget::model::{
    Budget, BudgetAdministrativeState, BudgetBalance, BudgetHold, BudgetHoldStatus, BudgetVersion,
};
use crate::telemetry::log_event;

use super::manager::BudgetVersionProvenance;

#[derive(Debug, Clone)]
pub(crate) struct BudgetState {
    budgets: HashMap<BudgetId, Budget>,
    budget_versions: HashMap<BudgetVersionId, BudgetVersion>,
    budget_version_id_by_revision: HashMap<(BudgetId, u64), BudgetVersionId>,
    successor_version_id_by_predecessor: HashMap<BudgetVersionId, BudgetVersionId>,
    budget_balances: HashMap<BudgetId, BudgetBalance>,
    budget_holds: HashMap<BudgetHoldId, BudgetHold>,
    hold_id_by_spend_decision: HashMap<SpendDecisionId, BudgetHoldId>,

    budget_ids_by_agent_id: HashMap<AgentId, Vec<BudgetId>>,
}

impl BudgetState {
    pub(super) fn new() -> Self {
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

    pub(crate) fn from_records(
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

    pub(super) fn apply_persisted_finalization(
        &mut self,
        hold: BudgetHold,
        balance: BudgetBalance,
    ) {
        let budget_id = hold.budget_id.clone();
        self.budget_holds.insert(hold.id.clone(), hold);
        self.budget_balances.insert(budget_id, balance);
    }

    /// Create one budget and initialize its cached balance.
    ///
    /// An agent may only have one budget for a currency at any point in time.
    /// Creation rejects periods that overlap an existing budget with the same
    /// agent and currency.
    #[cfg(test)]
    pub(super) fn create_single_budget(
        &mut self,
        request: CreateSingleBudgetRequest,
    ) -> Result<CreateSingleBudgetResponse, BudgetManagerError> {
        self.create_single_budget_with_provenance(request, BudgetVersionProvenance::default())
    }

    /// Create one logical budget, its immutable revision 1, and logical balance.
    ///
    /// Production callers must provide authenticated actor and source
    /// provenance for the version audit record.
    pub(crate) fn create_single_budget_with_provenance(
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

    /// Reserve budget for an approved spend decision.
    ///
    /// This method checks remaining balance and updates the hold and balance in
    /// one mutable operation. Wrap the manager in a process-level lock when it is
    /// shared across concurrent handlers.
    pub(super) fn reserve_budget(
        &mut self,
        request: ReserveBudgetRequest,
    ) -> Result<ReserveBudgetResponse, BudgetManagerError> {
        self.reserve_budget_at(request, Utc::now())
    }

    /// Reserve from a budget that is effectively active at `now`.
    pub(super) fn reserve_budget_at(
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
    pub(super) fn claim_budget(
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
    pub(super) fn settle_budget(
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
    pub(super) fn release_budget(
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
    pub(super) fn expire_overdue_budget_holds(
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

    pub(super) fn get_budget_by_id(&self, budget_id: &BudgetId) -> Option<BudgetWithBalance> {
        budget_with_balance(
            &self.budgets,
            &self.budget_versions,
            &self.budget_balances,
            budget_id,
        )
    }

    pub(super) fn get_budgets_by_agent_id(&self, agent_id: &AgentId) -> Vec<BudgetWithBalance> {
        self.budget_ids_by_agent_id
            .get(agent_id)
            .map(|budget_ids| self.budgets_with_balances(budget_ids))
            .unwrap_or_default()
    }

    /// Return the immutable version history for one logical budget in ascending
    /// revision order.
    pub(super) fn get_budget_versions_by_budget_id(
        &self,
        budget_id: &BudgetId,
    ) -> Vec<BudgetVersion> {
        let mut versions = self
            .budget_versions
            .values()
            .filter(|version| version.budget_id == *budget_id)
            .cloned()
            .collect::<Vec<_>>();
        versions.sort_by_key(|version| version.revision);
        versions
    }

    pub(super) fn get_evaluated_budget_by_id(
        &self,
        budget_id: &BudgetId,
        now: DateTime<Utc>,
    ) -> Result<Option<EvaluatedBudget>, BudgetManagerError> {
        self.get_budget_by_id(budget_id)
            .map(|budget| budget.evaluate_at(now).map_err(Into::into))
            .transpose()
    }

    pub(super) fn get_evaluated_budgets_by_agent_id(
        &self,
        agent_id: &AgentId,
        now: DateTime<Utc>,
    ) -> Result<Vec<EvaluatedBudget>, BudgetManagerError> {
        self.get_budgets_by_agent_id(agent_id)
            .into_iter()
            .map(|budget| budget.evaluate_at(now).map_err(Into::into))
            .collect()
    }

    pub(super) fn available_budget_id_for_agent_at(
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

    pub(super) fn get_budget_balance(&self, budget_id: &BudgetId) -> Option<BudgetBalance> {
        self.budget_balances.get(budget_id).cloned()
    }

    pub(super) fn get_budget_hold(&self, hold_id: &BudgetHoldId) -> Option<BudgetHold> {
        self.budget_holds.get(hold_id).cloned()
    }

    pub(super) fn get_budget_hold_by_spend_decision(
        &self,
        spend_decision_id: &SpendDecisionId,
    ) -> Option<BudgetHold> {
        self.hold_id_by_spend_decision
            .get(spend_decision_id)
            .and_then(|hold_id| self.get_budget_hold(hold_id))
    }

    #[cfg(test)]
    pub(super) fn revoke_budget_at(
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

impl Default for BudgetState {
    fn default() -> Self {
        Self::new()
    }
}

fn invalid_persisted_budget_state(message: impl Into<String>) -> BudgetManagerError {
    BudgetManagerError::InvalidPersistedState(message.into())
}
