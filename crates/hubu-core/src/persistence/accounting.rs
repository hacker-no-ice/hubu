//! Hubu's non-cash provider subledger. A journal row is a balanced posting pair;
//! the SQL view exposes its two lines without allowing partially written pairs.
use super::*;
use crate::spend::SpendExecutorVendorCost;
use hubu_common::ids::AgentAccountId;

const CENT_AT_SCALE_18: i128 = 10_000_000_000_000_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderAccountingRecord {
    pub id: String,
    pub original_entry_id: String,
    pub previous_entry_id: Option<String>,
    pub owner_user_id: UserId,
    pub agent_id: AgentId,
    pub agent_account_id: AgentAccountId,
    pub operation_key: String,
    pub budget_id: BudgetId,
    pub budget_version_id: BudgetVersionId,
    pub claim_id: SpendExecutorClaimId,
    pub settlement_id: PaymentId,
    pub provider: Option<String>,
    pub billing_merchant: Option<String>,
    pub purpose: Option<String>,
    pub provider_request_id: String,
    pub reconciliation_provider_reference: Option<String>,
    pub reconciliation_evidence: Option<String>,
    /// Immutable original receipt evidence, retained on every adjustment.
    pub receipt: SpendExecutorSettlementReceipt,
    /// Original exact cost, or corrected total cost after this adjustment.
    pub effective_vendor_cost: SpendExecutorVendorCost,
    /// Signed coefficient at scale 18, encoded as a string to preserve i128 precision.
    pub expense_delta_amount: String,
    pub budget_charge_delta_cents: i64,
    /// Signed budget charge minus exact expense, also at scale 18.
    pub rounding_delta_amount: String,
    pub reason: Option<String>,
    pub adjustment_evidence: Option<String>,
    pub adjustment_operation_key: Option<String>,
    pub legacy_backfill: bool,
    pub missing_evidence: Vec<String>,
    pub created_at: DateTime<Utc>,
}

/// Human-authorized correction command for the BudgetManager facade.
/// No executor or public transport endpoint accepts this command.
#[derive(Debug, Clone)]
pub struct ProviderAccountingAdjustment {
    pub owner_user_id: UserId,
    pub original_entry_id: String,
    pub expected_previous_entry_id: String,
    pub operation_key: String,
    pub corrected_vendor_cost: SpendExecutorVendorCost,
    pub reason: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderBudgetReconciliation {
    pub budget_id: BudgetId,
    pub consumed_amount_cents: i64,
    pub provider_budget_charges_cents: i64,
    /// Wallet consumption and/or missing legacy evidence. Never silently treated
    /// as provider expense or as proof that all consumption has ledger coverage.
    pub other_or_unaccounted_consumption_cents: i64,
}

fn invalid(message: &str) -> StorageError {
    StorageError::InvalidData(message.to_string())
}

fn exact_amount(cost: &SpendExecutorVendorCost) -> Result<i128, StorageError> {
    cost.conservative_budget_charge_cents().map_err(invalid)?;
    Ok(i128::from(cost.amount) * 10_i128.pow(18 - cost.scale))
}

fn insert_record(conn: &Connection, record: &ProviderAccountingRecord) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO provider_accounting_journal
         (id, owner_user_id, budget_id, original_entry_id, previous_entry_id,
          expense_delta_amount, budget_charge_delta_cents, currency, record_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            record.id,
            record.owner_user_id.to_string(),
            record.budget_id.to_string(),
            record.original_entry_id,
            record.previous_entry_id,
            record.expense_delta_amount,
            record.budget_charge_delta_cents,
            record.effective_vendor_cost.currency.to_string(),
            serde_json::to_string(record)?
        ],
    )?;
    Ok(())
}

fn load_record(
    conn: &Connection,
    id: &str,
) -> Result<Option<ProviderAccountingRecord>, StorageError> {
    let json: Option<String> = conn
        .query_row(
            "SELECT record_json FROM provider_accounting_journal WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    json.map(|value| serde_json::from_str(&value).map_err(Into::into))
        .transpose()
}

pub(super) fn post_settlement(
    conn: &Connection,
    claim: &SpendExecutorClaimRecord,
    hold: &BudgetHold,
    receipt: &PersistedSpendExecutorSettlementReceipt,
    legacy_backfill: bool,
) -> Result<(), StorageError> {
    let id = format!("settlement:{}", receipt.settlement_id);
    let json: Option<String> = conn
        .query_row(
            "SELECT d.request_json FROM spend_decisions d
             JOIN spend_auth_tokens t ON t.spend_decision_id = d.id
             JOIN spend_executor_claims c ON c.spend_auth_token_id = t.id
             JOIN budget_holds h ON h.spend_decision_id = d.id AND h.executor_claim_id = c.id
             WHERE c.id = ?1 AND h.id = ?2 AND c.status = 'settled' AND h.status = 'settled'
               AND c.settlement_id = ?3 AND t.used_by_payment_id = ?3 AND t.used_at IS NOT NULL
               AND d.operation_key = c.operation_key AND d.owner_user_id = c.owner_user_id
               AND d.agent_id = c.agent_id AND t.owner_user_id = c.owner_user_id",
            params![
                claim.id.to_string(),
                hold.id.to_string(),
                receipt.settlement_id.to_string()
            ],
            |row| row.get(0),
        )
        .optional()?;
    let json = json
        .ok_or_else(|| invalid("missing or inconsistent provider accounting settlement linkage"))?;
    let request: SpendRequest = serde_json::from_str(&json)?;
    if request.owner_user_id != claim.owner_user_id
        || request.agent_id != claim.agent_id
        || receipt.authorized_max_cents != hold.amount_cents
        || receipt.claim_id != claim.id
        || receipt.currency != hold.currency
        || receipt.receipt.actual_vendor_cost.currency != hold.currency
        || receipt.budget_charge_cents
            != receipt
                .receipt
                .actual_vendor_cost
                .conservative_budget_charge_cents()
                .map_err(invalid)?
    {
        return Err(invalid(
            "provider accounting evidence does not match settlement",
        ));
    }
    let provider = request
        .execution_scope
        .as_ref()
        .map(|scope| scope.provider.id.clone())
        .filter(|id| id != "provider:legacy:unresolved")
        .or_else(|| {
            receipt
                .receipt
                .price_model_snapshot
                .get("provider")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
    let billing_merchant = request
        .execution_scope
        .as_ref()
        .map(|scope| scope.billing_merchant.id.clone())
        .or(request.merchant);
    let purpose = (!request.reason.trim().is_empty()).then_some(request.reason);
    let mut missing_evidence = Vec::new();
    if provider.is_none() {
        missing_evidence.push("provider".to_string());
    }
    if billing_merchant.is_none() {
        missing_evidence.push("billing_merchant".to_string());
    }
    if purpose.is_none() {
        missing_evidence.push("purpose".to_string());
    }
    let expense = exact_amount(&receipt.receipt.actual_vendor_cost)?;
    let record = ProviderAccountingRecord {
        id: id.clone(),
        original_entry_id: id,
        previous_entry_id: None,
        owner_user_id: claim.owner_user_id.clone(),
        agent_id: claim.agent_id.clone(),
        agent_account_id: request.agent_account_id,
        operation_key: claim.operation_key.clone(),
        budget_id: hold.budget_id.clone(),
        budget_version_id: hold.budget_version_id.clone(),
        claim_id: claim.id.clone(),
        settlement_id: receipt.settlement_id.clone(),
        provider,
        billing_merchant,
        purpose,
        provider_request_id: receipt.receipt.provider_request_id.clone(),
        reconciliation_provider_reference: claim.provider_reference.clone(),
        reconciliation_evidence: claim.reconciliation_evidence.clone(),
        receipt: receipt.receipt.clone(),
        effective_vendor_cost: receipt.receipt.actual_vendor_cost.clone(),
        expense_delta_amount: expense.to_string(),
        budget_charge_delta_cents: receipt.budget_charge_cents,
        rounding_delta_amount: (i128::from(receipt.budget_charge_cents) * CENT_AT_SCALE_18
            - expense)
            .to_string(),
        reason: None,
        adjustment_evidence: None,
        adjustment_operation_key: None,
        legacy_backfill,
        missing_evidence,
        created_at: receipt.created_at,
    };
    insert_record(conn, &record)
}

impl SqliteGovernanceRepository {
    pub(super) fn initialize_provider_accounting(&mut self) -> Result<(), StorageError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS provider_accounting_journal (
                id TEXT PRIMARY KEY,
                owner_user_id TEXT NOT NULL,
                budget_id TEXT NOT NULL REFERENCES budgets(id),
                original_entry_id TEXT NOT NULL REFERENCES provider_accounting_journal(id),
                previous_entry_id TEXT UNIQUE REFERENCES provider_accounting_journal(id),
                expense_delta_amount TEXT NOT NULL,
                budget_charge_delta_cents INTEGER NOT NULL,
                currency TEXT NOT NULL,
                record_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS provider_accounting_owner_budget
                ON provider_accounting_journal(owner_user_id, budget_id);
            CREATE TRIGGER IF NOT EXISTS provider_accounting_no_update
                BEFORE UPDATE ON provider_accounting_journal BEGIN
                SELECT RAISE(ABORT, 'provider accounting is immutable'); END;
            CREATE TRIGGER IF NOT EXISTS provider_accounting_no_delete
                BEFORE DELETE ON provider_accounting_journal BEGIN
                SELECT RAISE(ABORT, 'provider accounting is immutable'); END;
            CREATE TABLE IF NOT EXISTS provider_accounting_legacy_gaps (
                claim_id TEXT PRIMARY KEY, reason TEXT NOT NULL
            );
            CREATE VIEW IF NOT EXISTS provider_accounting_lines AS
                SELECT id AS transaction_id, owner_user_id, budget_id,
                    'provider_spend_expense' AS account,
                    CASE WHEN substr(expense_delta_amount, 1, 1) = '-' THEN 'credit' ELSE 'debit' END AS direction,
                    ltrim(expense_delta_amount, '-') AS amount, 18 AS scale, currency
                FROM provider_accounting_journal
                UNION ALL
                SELECT id, owner_user_id, budget_id, 'externally_billed_clearing',
                    CASE WHEN substr(expense_delta_amount, 1, 1) = '-' THEN 'debit' ELSE 'credit' END,
                    ltrim(expense_delta_amount, '-'), 18, currency
                FROM provider_accounting_journal;"
        )?;
        let ids = {
            let mut stmt = tx.prepare("SELECT c.id FROM spend_executor_claims c
                WHERE c.status = 'settled' AND NOT EXISTS
                (SELECT 1 FROM provider_accounting_journal j WHERE j.id = 'settlement:' || c.settlement_id)
                UNION SELECT r.claim_id FROM spend_executor_settlement_receipts r
                LEFT JOIN spend_executor_claims c ON c.id = r.claim_id WHERE c.id IS NULL")?;
            let ids = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids
        };
        for id in ids {
            let claim_id = SpendExecutorClaimId::from_str(&id)
                .map_err(|_| invalid("invalid legacy claim id"))?;
            let Some(claim) = load_executor_claim_by_id(&tx, &claim_id)? else {
                tx.execute("INSERT OR IGNORE INTO provider_accounting_legacy_gaps (claim_id, reason) VALUES (?1, 'legacy receipt is missing its claim')", [&id])?;
                continue;
            };
            let hold = load_budget_hold_by_claim_id(&tx, &claim_id)?;
            let receipt = load_executor_settlement_receipt_by_claim_id(&tx, &claim_id)?;
            let gap = match (hold, receipt) {
                (Some(hold), Some(receipt))
                    if claim.settlement_id.as_ref() == Some(&receipt.settlement_id) =>
                {
                    match post_settlement(&tx, &claim, &hold, &receipt, true) {
                        Ok(()) => None,
                        Err(StorageError::InvalidData(_)) | Err(StorageError::Json { .. }) => {
                            Some("legacy settlement evidence is incomplete or inconsistent")
                        }
                        Err(error) => return Err(error),
                    }
                }
                _ => Some("legacy settled claim is missing matching receipt or budget hold"),
            };
            if let Some(reason) = gap {
                tx.execute("INSERT OR IGNORE INTO provider_accounting_legacy_gaps (claim_id, reason) VALUES (?1, ?2)", params![id, reason])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn provider_accounting_records(
        &self,
        owner_user_id: &UserId,
    ) -> Result<Vec<ProviderAccountingRecord>, StorageError> {
        let mut stmt = self.conn.prepare("SELECT record_json FROM provider_accounting_journal WHERE owner_user_id = ?1 ORDER BY rowid")?;
        let rows = stmt.query_map([owner_user_id.to_string()], |row| row.get::<_, String>(0))?;
        rows.map(|row| serde_json::from_str(&row?).map_err(Into::into))
            .collect()
    }

    /// Append a correction to a settled provider expense. Positive corrections
    /// record already-incurred cost even if the budget is exhausted or revoked.
    /// Negative corrections can only refund this settlement's previous charge.
    pub(crate) fn adjust_provider_accounting(
        &mut self,
        command: ProviderAccountingAdjustment,
    ) -> Result<(ProviderAccountingRecord, crate::budget::state::BudgetState), StorageError> {
        if command.operation_key.trim().is_empty()
            || command.reason.trim().is_empty()
            || command.evidence.trim().is_empty()
        {
            return Err(invalid(
                "adjustment requires operation key, reason and evidence",
            ));
        }
        let corrected_amount = exact_amount(&command.corrected_vendor_cost)?;
        let id = format!(
            "adjustment:{}:{:x}",
            command.owner_user_id,
            Sha256::digest(command.operation_key.as_bytes())
        );
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = load_record(&tx, &id)? {
            if existing.original_entry_id != command.original_entry_id
                || existing.previous_entry_id.as_deref()
                    != Some(&command.expected_previous_entry_id)
                || existing.effective_vendor_cost != command.corrected_vendor_cost
                || existing.reason.as_deref() != Some(&command.reason)
                || existing.adjustment_evidence.as_deref() != Some(&command.evidence)
            {
                return Err(invalid("conflicting provider accounting adjustment replay"));
            }
            let state = load_budget_state_from(&tx)?;
            tx.commit()?;
            return Ok((existing, state));
        }
        let original = load_record(&tx, &command.original_entry_id)?
            .ok_or_else(|| invalid("unknown original accounting entry"))?;
        let previous = load_record(&tx, &command.expected_previous_entry_id)?
            .ok_or_else(|| invalid("unknown previous accounting entry"))?;
        if original.owner_user_id != command.owner_user_id
            || original.previous_entry_id.is_some()
            || previous.original_entry_id != original.id
            || previous.owner_user_id != command.owner_user_id
            || command.corrected_vendor_cost.currency != original.effective_vendor_cost.currency
        {
            return Err(invalid("adjustment owner, source or currency mismatch"));
        }
        let has_successor: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM provider_accounting_journal WHERE previous_entry_id = ?1)",
            [&previous.id],
            |row| row.get(0),
        )?;
        if has_successor {
            return Err(invalid("adjustment predecessor is stale"));
        }
        let charge = command
            .corrected_vendor_cost
            .conservative_budget_charge_cents()
            .map_err(invalid)?;
        let old_charge = previous
            .effective_vendor_cost
            .conservative_budget_charge_cents()
            .map_err(invalid)?;
        let delta = charge - old_charge;
        let expense_delta = corrected_amount - exact_amount(&previous.effective_vendor_cost)?;
        let balance = load_budget_balance_by_id(&tx, &original.budget_id)?
            .ok_or_else(|| invalid("missing adjustment budget"))?;
        let consumed = balance
            .consumed_amount_cents
            .checked_add(delta)
            .filter(|amount| *amount >= 0)
            .ok_or_else(|| invalid("invalid adjusted consumption"))?;
        let remaining = balance
            .remaining_amount_cents
            .checked_sub(delta)
            .ok_or_else(|| invalid("adjusted remaining budget overflow"))?;
        let now = Utc::now();
        tx.execute("UPDATE budget_balances SET consumed_amount_cents = ?2, remaining_amount_cents = ?3, updated_at = ?4 WHERE budget_id = ?1", params![original.budget_id.to_string(), consumed, remaining, now.to_rfc3339()])?;
        let record = ProviderAccountingRecord {
            id,
            previous_entry_id: Some(previous.id),
            effective_vendor_cost: command.corrected_vendor_cost,
            expense_delta_amount: expense_delta.to_string(),
            budget_charge_delta_cents: delta,
            rounding_delta_amount: (i128::from(delta) * CENT_AT_SCALE_18 - expense_delta)
                .to_string(),
            reason: Some(command.reason),
            adjustment_evidence: Some(command.evidence),
            adjustment_operation_key: Some(command.operation_key),
            legacy_backfill: false,
            created_at: now,
            ..original
        };
        insert_record(&tx, &record)?;
        let state = load_budget_state_from(&tx)?;
        tx.commit()?;
        Ok((record, state))
    }

    pub fn reconcile_provider_budget(
        &self,
        owner_user_id: &UserId,
        budget_id: &BudgetId,
    ) -> Result<ProviderBudgetReconciliation, StorageError> {
        // Both reads share a SQLite snapshot even while another connection settles.
        let tx = self.conn.unchecked_transaction()?;
        let charges = {
            let mut stmt = tx.prepare("SELECT budget_charge_delta_cents FROM provider_accounting_journal WHERE owner_user_id = ?1 AND budget_id = ?2")?;
            let charges = stmt
                .query_map(
                    params![owner_user_id.to_string(), budget_id.to_string()],
                    |row| row.get::<_, i64>(0),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            charges
        };
        if charges.is_empty() {
            return Err(invalid("unknown provider accounting budget"));
        }
        let total: i128 = charges.into_iter().map(i128::from).sum();
        let total = i64::try_from(total).map_err(|_| invalid("provider charge total overflow"))?;
        let balance = load_budget_balance_by_id(&tx, budget_id)?
            .ok_or_else(|| invalid("missing accounting budget"))?;
        Ok(ProviderBudgetReconciliation {
            budget_id: budget_id.clone(),
            consumed_amount_cents: balance.consumed_amount_cents,
            provider_budget_charges_cents: total,
            other_or_unaccounted_consumption_cents: balance
                .consumed_amount_cents
                .checked_sub(total)
                .ok_or_else(|| invalid("reconciliation difference overflow"))?,
        })
    }
}
