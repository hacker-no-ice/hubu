use chrono::{DateTime, Utc};
use hubu_common::ids::{AgentId, BudgetHoldId, BudgetId, SpendDecisionId, SpendExecutorClaimId};
use hubu_common::money::Currency;

use super::state::BudgetState;
use crate::budget::dto::{
    BudgetWithBalance, CreateSingleBudgetRequest, CreateSingleBudgetResponse, EvaluatedBudget,
    ExpireBudgetHoldResponse, ReleaseBudgetResponse, ReserveBudgetRequest, ReserveBudgetResponse,
    SettleBudgetResponse,
};
use crate::budget::error::BudgetManagerError;
use crate::budget::model::{Budget, BudgetBalance, BudgetHold, BudgetVersion};

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

/// Public budget facade. Representation, hydration validation, and accounting
/// rules are owned by private state. Administration commits through its private
/// storage-first coordinator before changing memory.
#[derive(Debug)]
pub struct BudgetManager {
    state: BudgetState,
    coordinator: Option<super::coordinator::BudgetCoordinator>,
}

impl BudgetManager {
    pub(crate) fn accounting_repository(
        &self,
    ) -> Result<
        std::sync::Arc<std::sync::Mutex<crate::persistence::SqliteGovernanceRepository>>,
        crate::storage::StorageError,
    > {
        Ok(self.coordinator()?.repository)
    }

    pub fn new() -> Self {
        Self {
            state: BudgetState::new(),
            coordinator: None,
        }
    }

    pub fn from_records(
        budgets: Vec<Budget>,
        versions: Vec<BudgetVersion>,
        balances: Vec<BudgetBalance>,
        holds: Vec<BudgetHold>,
    ) -> Result<Self, BudgetManagerError> {
        BudgetState::from_records(budgets, versions, balances, holds).map(|state| Self {
            state,
            coordinator: None,
        })
    }

    pub(crate) fn apply_committed_state(&mut self, state: BudgetState) {
        self.state = state;
    }

    pub(crate) fn apply_persisted_finalization(
        &mut self,
        hold: BudgetHold,
        balance: BudgetBalance,
    ) {
        self.state.apply_persisted_finalization(hold, balance)
    }

    /// Bind administration to the shared governance repository. All users must
    /// acquire the manager before the repository; commands lock it internally.
    pub fn with_repository(
        mut self,
        repository: std::sync::Arc<
            std::sync::Mutex<crate::persistence::SqliteGovernanceRepository>,
        >,
    ) -> Self {
        self.coordinator = Some(super::coordinator::BudgetCoordinator { repository });
        self
    }

    fn coordinator(
        &self,
    ) -> Result<super::coordinator::BudgetCoordinator, crate::storage::StorageError> {
        self.coordinator.clone().ok_or_else(|| {
            crate::storage::StorageError::InvalidData(
                "budget administration requires a configured repository".into(),
            )
        })
    }

    #[cfg(test)]
    pub(crate) fn create_single_budget(
        &mut self,
        request: CreateSingleBudgetRequest,
    ) -> Result<CreateSingleBudgetResponse, BudgetManagerError> {
        self.state.create_single_budget(request)
    }

    #[cfg(test)]
    pub(crate) fn create_single_budget_in_memory(
        &mut self,
        request: CreateSingleBudgetRequest,
        provenance: BudgetVersionProvenance,
    ) -> Result<CreateSingleBudgetResponse, BudgetManagerError> {
        self.state
            .create_single_budget_with_provenance(request, provenance)
    }

    #[cfg(test)]
    pub(crate) fn revoke_budget_in_memory(
        &mut self,
        budget_id: &BudgetId,
    ) -> Result<BudgetWithBalance, BudgetManagerError> {
        self.state.revoke_budget_at(budget_id, Utc::now())
    }

    #[cfg(test)]
    pub(crate) fn revoke_budget_at_in_memory(
        &mut self,
        budget_id: &BudgetId,
        now: DateTime<Utc>,
    ) -> Result<BudgetWithBalance, BudgetManagerError> {
        self.state.revoke_budget_at(budget_id, now)
    }

    /// Commit the initial version, logical budget and balance before publishing
    /// them to the manager. A missing repository is an explicit error.
    pub fn create_single_budget_with_provenance(
        &mut self,
        request: CreateSingleBudgetRequest,
        provenance: BudgetVersionProvenance,
    ) -> Result<CreateSingleBudgetResponse, BudgetManagerError> {
        self.coordinator()?.create(self, request, provenance)
    }

    /// Append or exactly replay a limit update under the shared repository lock.
    pub fn update_limit(
        &mut self,
        request: super::UpdateBudgetLimitRequest,
        effective_at: DateTime<Utc>,
    ) -> Result<super::UpdateBudgetLimitResponse, super::BudgetLimitUpdateError> {
        self.coordinator()
            .map_err(super::BudgetUpdateError::from)?
            .update(self, request, effective_at)
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
        self.state.reserve_budget(request)
    }

    /// Reserve from a budget that is effectively active at `now`.
    pub fn reserve_budget_at(
        &mut self,
        request: ReserveBudgetRequest,
        now: DateTime<Utc>,
    ) -> Result<ReserveBudgetResponse, BudgetManagerError> {
        self.state.reserve_budget_at(request, now)
    }

    /// Bind a frozen hold to one executor claim and extend its execution lease.
    pub fn claim_budget(
        &mut self,
        hold_id: &BudgetHoldId,
        claim_id: SpendExecutorClaimId,
        expires_at: DateTime<Utc>,
    ) -> Result<ReserveBudgetResponse, BudgetManagerError> {
        self.state.claim_budget(hold_id, claim_id, expires_at)
    }

    /// Settle a frozen budget hold after payment succeeds.
    pub fn settle_budget(
        &mut self,
        hold_id: &BudgetHoldId,
    ) -> Result<SettleBudgetResponse, BudgetManagerError> {
        self.state.settle_budget(hold_id)
    }

    /// Release a frozen budget hold back into remaining budget.
    pub fn release_budget(
        &mut self,
        hold_id: &BudgetHoldId,
    ) -> Result<ReleaseBudgetResponse, BudgetManagerError> {
        self.state.release_budget(hold_id)
    }

    /// Expire frozen holds whose authorization window has passed.
    pub fn expire_overdue_budget_holds(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<Vec<ExpireBudgetHoldResponse>, BudgetManagerError> {
        self.state.expire_overdue_budget_holds(now)
    }

    pub fn get_budget_by_id(&self, budget_id: &BudgetId) -> Option<BudgetWithBalance> {
        self.state.get_budget_by_id(budget_id)
    }

    pub fn get_budgets_by_agent_id(&self, agent_id: &AgentId) -> Vec<BudgetWithBalance> {
        self.state.get_budgets_by_agent_id(agent_id)
    }

    /// Return the immutable version history for one logical budget in ascending
    /// revision order.
    pub fn get_budget_versions_by_budget_id(&self, budget_id: &BudgetId) -> Vec<BudgetVersion> {
        self.state.get_budget_versions_by_budget_id(budget_id)
    }

    pub fn get_evaluated_budget_by_id(
        &self,
        budget_id: &BudgetId,
        now: DateTime<Utc>,
    ) -> Result<Option<EvaluatedBudget>, BudgetManagerError> {
        self.state.get_evaluated_budget_by_id(budget_id, now)
    }

    pub fn get_evaluated_budgets_by_agent_id(
        &self,
        agent_id: &AgentId,
        now: DateTime<Utc>,
    ) -> Result<Vec<EvaluatedBudget>, BudgetManagerError> {
        self.state.get_evaluated_budgets_by_agent_id(agent_id, now)
    }

    pub fn available_budget_id_for_agent_at(
        &self,
        agent_id: &AgentId,
        currency: Currency,
        now: DateTime<Utc>,
    ) -> Result<Option<BudgetId>, BudgetManagerError> {
        self.state
            .available_budget_id_for_agent_at(agent_id, currency, now)
    }

    pub fn get_budget_balance(&self, budget_id: &BudgetId) -> Option<BudgetBalance> {
        self.state.get_budget_balance(budget_id)
    }

    pub fn get_budget_hold(&self, hold_id: &BudgetHoldId) -> Option<BudgetHold> {
        self.state.get_budget_hold(hold_id)
    }

    pub fn get_budget_hold_by_spend_decision(
        &self,
        spend_decision_id: &SpendDecisionId,
    ) -> Option<BudgetHold> {
        self.state
            .get_budget_hold_by_spend_decision(spend_decision_id)
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
        self.coordinator()?.revoke(self, budget_id, now)
    }
}

impl Default for BudgetManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use chrono::TimeZone;

    use super::*;
    use crate::budget::{BudgetAdministrativeState, BudgetAvailability, BudgetHoldStatus};
    use hubu_common::ids::BudgetVersionId;
    use hubu_common::time::TimePeriod;

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

    // Separate managers let hydration tests assemble conflicting persisted records
    // through the supported facade without exposing private construction helpers.
    fn create_isolated_budget(
        agent_id: AgentId,
        amount_limit_cents: i64,
        currency: Currency,
        period: TimePeriod,
        provenance: &BudgetVersionProvenance,
    ) -> Result<BudgetWithBalance, BudgetManagerError> {
        let created = BudgetManager::new().create_single_budget_in_memory(
            CreateSingleBudgetRequest {
                agent_id,
                amount_limit_cents,
                currency,
                period,
            },
            provenance.clone(),
        )?;
        Ok(BudgetWithBalance {
            budget: created.budget,
            version: created.version,
            balance: created.balance,
        })
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
        let first = create_isolated_budget(
            agent_id.clone(),
            1_000,
            Currency::Usd,
            TimePeriod::new(start, Some(start + Duration::hours(2))).unwrap(),
            &provenance,
        )
        .unwrap();
        let second = create_isolated_budget(
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
        let first = create_isolated_budget(
            agent_id.clone(),
            1_000,
            Currency::Usd,
            TimePeriod::new(start, Some(boundary)).unwrap(),
            &provenance,
        )
        .unwrap();
        let second = create_isolated_budget(
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

        let mut revoked_overlap = create_isolated_budget(
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
            .revoke_budget_in_memory(&created.budget.id)
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
            .revoke_budget_in_memory(&created.budget.id)
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
            .revoke_budget_in_memory(&created.budget.id)
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
            .revoke_budget_at_in_memory(&scheduled.budget.id, start + Duration::minutes(3))
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
        let budgets: Vec<_> = (0..2)
            .map(|day| {
                manager
                    .create_single_budget(CreateSingleBudgetRequest {
                        agent_id: agent_id.clone(),
                        amount_limit_cents: 1_000,
                        currency: Currency::Usd,
                        period: TimePeriod::new(
                            timestamp() + Duration::days(day),
                            Some(timestamp() + Duration::days(day + 1)),
                        )
                        .unwrap(),
                    })
                    .unwrap()
            })
            .collect();
        let boundary = budgets[1].budget.period.starting_at;

        assert_eq!(
            manager
                .available_budget_id_for_agent_at(
                    &agent_id,
                    Currency::Usd,
                    boundary - Duration::nanoseconds(1),
                )
                .unwrap(),
            Some(budgets[0].budget.id.clone())
        );
        assert_eq!(
            manager
                .available_budget_id_for_agent_at(&agent_id, Currency::Usd, boundary)
                .unwrap(),
            Some(budgets[1].budget.id.clone())
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
            .get_budget_hold(&reservation.hold.id)
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
