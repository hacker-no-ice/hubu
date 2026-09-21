//! First-class accounting facade. The ledger owns transaction identity, source
//! evidence, querying and corrections; budgets are linked control context.
use crate::{
    budget::BudgetManager, persistence::SqliteGovernanceRepository, storage::StorageError,
};
use hubu_common::ids::{AgentId, BudgetId, LedgerTransactionId, UserId};
pub use hubu_ledger::{
    ExactMoney, GovernedSpendContext, TransactionEntry, TransactionKind, TransactionMetadata,
    TransactionRecord,
};

#[derive(Debug, Clone)]
pub struct AccountingAdjustment {
    pub owner_user_id: UserId,
    pub original_transaction_id: LedgerTransactionId,
    pub expected_previous_transaction_id: LedgerTransactionId,
    pub operation_key: String,
    pub corrected_cost: ExactMoney,
    pub reason: String,
    pub evidence: String,
}
#[derive(Debug, Clone)]
pub struct BudgetLedgerQuery {
    pub owner_user_id: UserId,
    pub agent_id: AgentId,
    pub budget_id: BudgetId,
}
#[derive(Debug, Clone)]
pub struct BudgetLedgerReport {
    pub budget_id: BudgetId,
    pub agent_id: AgentId,
    pub transactions: Vec<TransactionRecord>,
    pub consumed_amount_cents: i64,
    pub recorded_budget_charges_cents: i64,
    pub unaccounted_consumption_cents: i64,
    /// Owner records without budget links are not attributed to the selected budget.
    pub owner_transactions_without_budget_context: usize,
    pub pending_hold_count: i64,
}
pub struct LedgerService;
impl LedgerService {
    /// Only externally billed provider corrections are supported. Wallet cash
    /// refunds require a separate confirmed rail flow.
    /// Caller authorizes the human correction and holds the budget-manager lock.
    /// Lock order is manager -> shared governance -> SQLite. Publication follows
    /// commit and happens while governance remains locked, including on replay.
    pub fn adjust(
        manager: &mut BudgetManager,
        command: AccountingAdjustment,
    ) -> Result<TransactionRecord, StorageError> {
        let shared = manager.accounting_repository()?;
        let mut repository = shared
            .lock()
            .map_err(|_| StorageError::InvalidData("governance store lock poisoned".into()))?;
        let (record, state) = repository.adjust_accounting(command)?;
        manager.apply_committed_state(state);
        Ok(record)
    }
    pub fn transactions_for_budget(
        repository: &SqliteGovernanceRepository,
        query: BudgetLedgerQuery,
    ) -> Result<BudgetLedgerReport, StorageError> {
        repository.query_budget_ledger(query)
    }
}
