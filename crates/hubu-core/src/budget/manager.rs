use std::collections::HashMap;

use chrono::{DateTime, Duration, Months, Utc};
use hubu_common::ids::{AgentId, BudgetHoldId, BudgetId, SpendDecisionId, TaskId, UserId};
use hubu_common::money::Currency;
use hubu_common::time::TimePeriod;

use crate::budget::dto::{
    BudgetRecurrence, BudgetWithBalance, CreateBudgetSeriesRequest, CreateBudgetSeriesResponse,
    CreateSingleBudgetRequest, CreateSingleBudgetResponse, ReleaseBudgetResponse,
    ReserveBudgetRequest, ReserveBudgetResponse, SettleBudgetResponse,
};
use crate::budget::error::BudgetManagerError;
use crate::budget::model::{
    Budget, BudgetBalance, BudgetHold, BudgetHoldStatus, BudgetScope, BudgetStatus,
};

pub struct BudgetManager {
    budgets: HashMap<BudgetId, Budget>,
    budget_balances: HashMap<BudgetId, BudgetBalance>,
    budget_holds: HashMap<BudgetHoldId, BudgetHold>,
    hold_by_spend_decision: HashMap<SpendDecisionId, BudgetHoldId>,

    budget_ids_by_user_id: HashMap<UserId, Vec<BudgetId>>,
    budget_ids_by_agent_id: HashMap<AgentId, Vec<BudgetId>>,
    budget_ids_by_task_id: HashMap<TaskId, Vec<BudgetId>>,
}

impl BudgetManager {
    pub fn new() -> Self {
        Self {
            budgets: HashMap::new(),
            budget_balances: HashMap::new(),
            budget_holds: HashMap::new(),
            hold_by_spend_decision: HashMap::new(),
            budget_ids_by_user_id: HashMap::new(),
            budget_ids_by_agent_id: HashMap::new(),
            budget_ids_by_task_id: HashMap::new(),
        }
    }

    /// Create one budget and initialize its cached balance.
    ///
    /// A scope may only have one budget for a currency at any point in time.
    /// Creation rejects periods that overlap an existing budget with the same
    /// scope and currency.
    pub fn create_single_budget(
        &mut self,
        request: CreateSingleBudgetRequest,
    ) -> Result<CreateSingleBudgetResponse, BudgetManagerError> {
        self.create_budget_for_period(
            request.scope,
            request.amount_limit_cents,
            request.currency,
            request.period,
        )
    }

    /// Create a finite recurring budget series.
    ///
    /// Each generated budget has its own balance. Consecutive periods use
    /// half-open boundaries, so the end of one period is the start of the next.
    /// The series is rejected if any generated period overlaps an existing
    /// budget with the same scope and currency.
    pub fn create_budget_series(
        &mut self,
        request: CreateBudgetSeriesRequest,
    ) -> Result<CreateBudgetSeriesResponse, BudgetManagerError> {
        if request.period_count == 0 {
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
            .any(|period| self.has_overlapping_budget(&request.scope, request.currency, period))
        {
            return Err(BudgetManagerError::OverlappingBudgetPeriod);
        }

        let mut budgets = Vec::with_capacity(request.period_count);
        for period in periods {
            let budget_with_balance = build_budget_for_period(
                request.scope.clone(),
                request.amount_limit_cents,
                request.currency,
                period,
            )?;
            budgets.push(budget_with_balance);
        }

        for budget_with_balance in &budgets {
            self.insert_budget(budget_with_balance);
        }

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
        if request.amount_cents <= 0 {
            return Err(BudgetManagerError::AmountMustBePositive);
        }

        if self
            .hold_by_spend_decision
            .contains_key(&request.spend_decision_id)
        {
            return Err(BudgetManagerError::DuplicateSpendDecisionHold);
        }

        let budget = self
            .budgets
            .get(&request.budget_id)
            .ok_or(BudgetManagerError::UnknownBudget)?;

        if !matches!(budget.status, BudgetStatus::Active) {
            return Err(BudgetManagerError::BudgetNotActive);
        }

        if !budget.period.contains(Utc::now()) {
            return Err(BudgetManagerError::BudgetPeriodInactive);
        }

        if budget.currency != request.currency {
            return Err(BudgetManagerError::CurrencyMismatch);
        }

        let balance = self
            .budget_balances
            .get_mut(&request.budget_id)
            .ok_or(BudgetManagerError::MissingBudgetBalance)?;

        if balance.remaining_amount_cents < request.amount_cents {
            return Err(BudgetManagerError::InsufficientRemainingBudget);
        }

        balance.remaining_amount_cents -= request.amount_cents;
        balance.frozen_amount_cents += request.amount_cents;

        let now = Utc::now();
        let hold = BudgetHold {
            id: BudgetHoldId::new(),
            budget_id: request.budget_id,
            spend_decision_id: request.spend_decision_id,
            amount_cents: request.amount_cents,
            currency: request.currency,
            status: BudgetHoldStatus::Frozen,
            created_at: now,
            updated_at: now,
            expires_at: request.expires_at,
        };

        self.hold_by_spend_decision
            .insert(hold.spend_decision_id.clone(), hold.id.clone());
        self.budget_holds.insert(hold.id.clone(), hold.clone());

        Ok(ReserveBudgetResponse {
            hold,
            balance: balance.clone(),
        })
    }

    /// Settle a frozen budget hold after payment succeeds.
    pub fn settle_budget(
        &mut self,
        hold_id: &BudgetHoldId,
    ) -> Result<SettleBudgetResponse, BudgetManagerError> {
        let hold = self
            .budget_holds
            .get_mut(hold_id)
            .ok_or(BudgetManagerError::UnknownBudgetHold)?;

        let balance = self
            .budget_balances
            .get_mut(&hold.budget_id)
            .ok_or(BudgetManagerError::MissingBudgetBalance)?;

        hold.settle()?;
        balance.frozen_amount_cents -= hold.amount_cents;
        balance.consumed_amount_cents += hold.amount_cents;

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
        let hold = self
            .budget_holds
            .get_mut(hold_id)
            .ok_or(BudgetManagerError::UnknownBudgetHold)?;

        let balance = self
            .budget_balances
            .get_mut(&hold.budget_id)
            .ok_or(BudgetManagerError::MissingBudgetBalance)?;

        hold.release()?;
        balance.frozen_amount_cents -= hold.amount_cents;
        balance.remaining_amount_cents += hold.amount_cents;

        Ok(ReleaseBudgetResponse {
            hold: hold.clone(),
            balance: balance.clone(),
        })
    }

    pub fn get_budget_by_id(&self, budget_id: &BudgetId) -> Option<BudgetWithBalance> {
        budget_with_balance(&self.budgets, &self.budget_balances, budget_id)
    }

    pub fn get_budgets_by_user_id(&self, user_id: &UserId) -> Vec<BudgetWithBalance> {
        self.budget_ids_by_user_id
            .get(user_id)
            .map(|budget_ids| self.budgets_with_balances(budget_ids))
            .unwrap_or_default()
    }

    pub fn get_budgets_by_agent_id(&self, agent_id: &AgentId) -> Vec<BudgetWithBalance> {
        self.budget_ids_by_agent_id
            .get(agent_id)
            .map(|budget_ids| self.budgets_with_balances(budget_ids))
            .unwrap_or_default()
    }

    pub fn get_budgets_by_task_id(&self, task_id: &TaskId) -> Vec<BudgetWithBalance> {
        self.budget_ids_by_task_id
            .get(task_id)
            .map(|budget_ids| self.budgets_with_balances(budget_ids))
            .unwrap_or_default()
    }

    pub fn get_budget_balance(&self, budget_id: &BudgetId) -> Option<BudgetBalance> {
        self.budget_balances.get(budget_id).cloned()
    }

    fn create_budget_for_period(
        &mut self,
        scope: BudgetScope,
        amount_limit_cents: i64,
        currency: Currency,
        period: TimePeriod,
    ) -> Result<CreateSingleBudgetResponse, BudgetManagerError> {
        if self.has_overlapping_budget(&scope, currency, &period) {
            return Err(BudgetManagerError::OverlappingBudgetPeriod);
        }

        let budget_with_balance =
            build_budget_for_period(scope, amount_limit_cents, currency, period)?;
        self.insert_budget(&budget_with_balance);

        Ok(CreateSingleBudgetResponse {
            budget: budget_with_balance.budget,
            balance: budget_with_balance.balance,
        })
    }

    fn insert_budget(&mut self, budget_with_balance: &BudgetWithBalance) {
        let budget = &budget_with_balance.budget;

        self.index_budget(budget);
        self.budget_balances
            .insert(budget.id.clone(), budget_with_balance.balance.clone());
        self.budgets.insert(budget.id.clone(), budget.clone());
    }

    fn budgets_with_balances(&self, budget_ids: &[BudgetId]) -> Vec<BudgetWithBalance> {
        budget_ids
            .iter()
            .filter_map(|budget_id| {
                budget_with_balance(&self.budgets, &self.budget_balances, budget_id)
            })
            .collect()
    }

    fn index_budget(&mut self, budget: &Budget) {
        match &budget.scope {
            BudgetScope::User(user_id) => self
                .budget_ids_by_user_id
                .entry(user_id.clone())
                .or_default()
                .push(budget.id.clone()),
            BudgetScope::Agent(agent_id) => self
                .budget_ids_by_agent_id
                .entry(agent_id.clone())
                .or_default()
                .push(budget.id.clone()),
            BudgetScope::Task(task_id) => self
                .budget_ids_by_task_id
                .entry(task_id.clone())
                .or_default()
                .push(budget.id.clone()),
        }
    }

    fn has_overlapping_budget(
        &self,
        scope: &BudgetScope,
        currency: Currency,
        period: &TimePeriod,
    ) -> bool {
        self.budgets.values().any(|budget| {
            budget.currency == currency
                && scopes_match(&budget.scope, scope)
                && periods_overlap(&budget.period, period)
        })
    }
}

fn scopes_match(left: &BudgetScope, right: &BudgetScope) -> bool {
    match (left, right) {
        (BudgetScope::User(left), BudgetScope::User(right)) => left == right,
        (BudgetScope::Agent(left), BudgetScope::Agent(right)) => left == right,
        (BudgetScope::Task(left), BudgetScope::Task(right)) => left == right,
        _ => false,
    }
}

fn periods_overlap(left: &TimePeriod, right: &TimePeriod) -> bool {
    let left_starts_before_right_ends = right
        .ending_before
        .map_or(true, |right_end| left.starting_at < right_end);
    let right_starts_before_left_ends = left
        .ending_before
        .map_or(true, |left_end| right.starting_at < left_end);

    left_starts_before_right_ends && right_starts_before_left_ends
}

fn build_budget_for_period(
    scope: BudgetScope,
    amount_limit_cents: i64,
    currency: Currency,
    period: TimePeriod,
) -> Result<BudgetWithBalance, BudgetManagerError> {
    let budget = Budget::new(BudgetId::new(), scope, amount_limit_cents, currency, period)?;
    let balance = BudgetBalance {
        budget_id: budget.id.clone(),
        consumed_amount_cents: 0,
        frozen_amount_cents: 0,
        remaining_amount_cents: budget.amount_limit_cents,
    };

    Ok(BudgetWithBalance { budget, balance })
}

fn budget_with_balance(
    budgets: &HashMap<BudgetId, Budget>,
    budget_balances: &HashMap<BudgetId, BudgetBalance>,
    budget_id: &BudgetId,
) -> Option<BudgetWithBalance> {
    Some(BudgetWithBalance {
        budget: budgets.get(budget_id)?.clone(),
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

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

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

    fn create_user_budget(manager: &mut BudgetManager, amount_cents: i64) -> BudgetWithBalance {
        let response = manager
            .create_single_budget(CreateSingleBudgetRequest {
                scope: BudgetScope::User(UserId::new()),
                amount_limit_cents: amount_cents,
                currency: Currency::Usd,
                period: active_period(),
            })
            .expect("budget should be created");

        BudgetWithBalance {
            budget: response.budget,
            balance: response.balance,
        }
    }

    #[test]
    fn create_single_budget_initializes_balance() {
        let mut manager = BudgetManager::new();

        let created = create_user_budget(&mut manager, 10_000);

        assert_eq!(created.balance.budget_id, created.budget.id);
        assert_eq!(created.balance.consumed_amount_cents, 0);
        assert_eq!(created.balance.frozen_amount_cents, 0);
        assert_eq!(created.balance.remaining_amount_cents, 10_000);
        assert!(manager.get_budget_by_id(&created.budget.id).is_some());
    }

    #[test]
    fn create_budget_series_creates_adjacent_periods() {
        let mut manager = BudgetManager::new();

        let response = manager
            .create_budget_series(CreateBudgetSeriesRequest {
                scope: BudgetScope::User(UserId::new()),
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
    fn create_budget_series_rejects_overlap_without_partial_creation() {
        let mut manager = BudgetManager::new();
        let user_id = UserId::new();

        manager
            .create_single_budget(CreateSingleBudgetRequest {
                scope: BudgetScope::User(user_id.clone()),
                amount_limit_cents: 10_000,
                currency: Currency::Usd,
                period: period(2026, 7, 15, 2026, 8, 15),
            })
            .expect("existing budget should be created");

        let error = manager
            .create_budget_series(CreateBudgetSeriesRequest {
                scope: BudgetScope::User(user_id.clone()),
                amount_limit_cents: 25_000,
                currency: Currency::Usd,
                starting_at: timestamp(),
                recurrence: BudgetRecurrence::Monthly,
                period_count: 2,
            })
            .expect_err("series should be rejected before creating any budget");

        assert!(matches!(error, BudgetManagerError::OverlappingBudgetPeriod));
        assert_eq!(manager.get_budgets_by_user_id(&user_id).len(), 1);
    }

    #[test]
    fn create_budget_series_rejects_invalid_budget_without_partial_creation() {
        let mut manager = BudgetManager::new();
        let user_id = UserId::new();

        let error = manager
            .create_budget_series(CreateBudgetSeriesRequest {
                scope: BudgetScope::User(user_id.clone()),
                amount_limit_cents: 0,
                currency: Currency::Usd,
                starting_at: timestamp(),
                recurrence: BudgetRecurrence::Monthly,
                period_count: 2,
            })
            .expect_err("invalid series should be rejected before creating any budget");

        assert!(matches!(error, BudgetManagerError::InvalidBudget(_)));
        assert!(manager.get_budgets_by_user_id(&user_id).is_empty());
    }

    #[test]
    fn create_single_budget_rejects_overlap_for_same_scope_and_currency() {
        let mut manager = BudgetManager::new();
        let user_id = UserId::new();

        manager
            .create_single_budget(CreateSingleBudgetRequest {
                scope: BudgetScope::User(user_id.clone()),
                amount_limit_cents: 10_000,
                currency: Currency::Usd,
                period: period(2026, 6, 1, 2026, 7, 1),
            })
            .expect("first budget should be created");

        let error = manager
            .create_single_budget(CreateSingleBudgetRequest {
                scope: BudgetScope::User(user_id),
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
        let user_id = UserId::new();

        manager
            .create_single_budget(CreateSingleBudgetRequest {
                scope: BudgetScope::User(user_id.clone()),
                amount_limit_cents: 10_000,
                currency: Currency::Usd,
                period: period(2026, 6, 1, 2026, 7, 1),
            })
            .expect("first budget should be created");

        manager
            .create_single_budget(CreateSingleBudgetRequest {
                scope: BudgetScope::User(user_id),
                amount_limit_cents: 10_000,
                currency: Currency::Usd,
                period: period(2026, 7, 1, 2026, 8, 1),
            })
            .expect("adjacent budget should be created");
    }

    #[test]
    fn create_single_budget_rejects_overlap_with_open_ended_period() {
        let mut manager = BudgetManager::new();
        let user_id = UserId::new();
        let open_ended_period =
            TimePeriod::new(Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap(), None).unwrap();

        manager
            .create_single_budget(CreateSingleBudgetRequest {
                scope: BudgetScope::User(user_id.clone()),
                amount_limit_cents: 10_000,
                currency: Currency::Usd,
                period: open_ended_period,
            })
            .expect("open-ended budget should be created");

        let error = manager
            .create_single_budget(CreateSingleBudgetRequest {
                scope: BudgetScope::User(user_id),
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
        let created = create_user_budget(&mut manager, 10_000);

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
        assert_eq!(response.balance.frozen_amount_cents, 3_000);
        assert_eq!(response.balance.remaining_amount_cents, 7_000);
        assert_eq!(response.balance.consumed_amount_cents, 0);
    }

    #[test]
    fn reserve_budget_rejects_duplicate_spend_decision() {
        let mut manager = BudgetManager::new();
        let created = create_user_budget(&mut manager, 10_000);
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
        let created = create_user_budget(&mut manager, 10_000);

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
        let created = create_user_budget(&mut manager, 10_000);
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
    fn release_budget_moves_frozen_amount_to_remaining() {
        let mut manager = BudgetManager::new();
        let created = create_user_budget(&mut manager, 10_000);
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
}
