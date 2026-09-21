//! Governance adapter for the independent ledger domain. All postings use the
//! canonical ledger transaction and entry tables shared with wallet payments.
use super::*;
use crate::ledger::{AccountingAdjustment, BudgetLedgerQuery, BudgetLedgerReport};
use hubu_common::ids::LedgerTransactionId;
use hubu_ledger::{
    domain as ledger, ExactMoney, GovernedSpendContext, LedgerAccountKind, TransactionKind,
    TransactionMetadata, TransactionRecord, CENT_AT_SCALE_18,
};

fn invalid(message: &str) -> StorageError {
    StorageError::InvalidData(message.to_string())
}
fn ledger_error(error: hubu_ledger::LedgerError) -> StorageError {
    match error {
        hubu_ledger::LedgerError::Sqlite { source } => StorageError::from(source),
        hubu_ledger::LedgerError::Json(source) => StorageError::from(source),
        error => invalid(&error.to_string()),
    }
}
fn exact_cost(cost: &crate::spend::SpendExecutorVendorCost) -> ExactMoney {
    ExactMoney {
        amount: cost.amount.to_string(),
        scale: cost.scale,
        currency: cost.currency,
    }
}

pub(super) fn post_settlement(
    conn: &rusqlite::Transaction<'_>,
    claim: &SpendExecutorClaimRecord,
    hold: &BudgetHold,
    receipt: &PersistedSpendExecutorSettlementReceipt,
    legacy_backfill: bool,
) -> Result<(), StorageError> {
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
        .or(request.merchant.clone());
    let purpose = (!request.reason.trim().is_empty()).then_some(request.reason.clone());
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
    let cost = exact_cost(&receipt.receipt.actual_vendor_cost);
    let expense = cost.scaled().map_err(ledger_error)?;
    let metadata = TransactionMetadata {
        kind: TransactionKind::ExternalProvider,
        source_key: format!("settlement:{}", receipt.settlement_id),
        agent_id: Some(request.agent_id.clone()),
        agent_account_id: Some(request.agent_account_id.clone()),
        context: Some(GovernedSpendContext {
            agent_id: claim.agent_id.clone(),
            agent_account_id: request.agent_account_id,
            budget_id: hold.budget_id.clone(),
            budget_version_id: hold.budget_version_id.clone(),
            spend_decision_id: hold.spend_decision_id.clone(),
            operation_key: claim.operation_key.clone(),
        }),
        effective_cost: cost,
        budget_charge_delta_cents: Some(receipt.budget_charge_cents),
        rounding_delta_amount: Some(
            (i128::from(receipt.budget_charge_cents) * CENT_AT_SCALE_18 - expense).to_string(),
        ),
        original_transaction_id: None,
        previous_transaction_id: None,
        provider,
        billing_merchant,
        purpose,
        source_evidence: serde_json::json!({"claim_id": claim.id, "settlement_id": receipt.settlement_id,
            "receipt": receipt.receipt, "reconciliation_provider_reference": claim.provider_reference,
            "reconciliation_evidence": claim.reconciliation_evidence}),
        reason: None,
        adjustment_evidence: None,
        legacy_backfill,
        missing_evidence,
    };
    ledger::post_pair(
        conn,
        ledger::PairPosting {
            owner: &claim.owner_user_id,
            debit_kind: LedgerAccountKind::AgentSpendExpense,
            credit_kind: LedgerAccountKind::ExternallyBilledClearing,
            signed_amount: expense,
            metadata,
            external_ref: Some(receipt.settlement_id.to_string()),
            created_at: receipt.created_at,
        },
    )
    .map_err(ledger_error)?;
    Ok(())
}

impl SqliteGovernanceRepository {
    pub(super) fn initialize_provider_accounting(&mut self) -> Result<(), StorageError> {
        ledger::initialize(&self.conn).map_err(ledger_error)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS ledger_legacy_gaps (source_key TEXT PRIMARY KEY, reason TEXT NOT NULL);")?;
        let ids = {
            let mut stmt = tx.prepare("SELECT c.id FROM spend_executor_claims c WHERE c.status='settled'
                AND NOT EXISTS (SELECT 1 FROM ledger_transaction_metadata m WHERE m.source_key='settlement:' || c.settlement_id)
                UNION SELECT r.claim_id FROM spend_executor_settlement_receipts r LEFT JOIN spend_executor_claims c ON c.id=r.claim_id WHERE c.id IS NULL")?;
            let ids = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids
        };
        for id in ids {
            let claim_id = id.parse().map_err(|_| invalid("invalid legacy claim id"))?;
            let claim = load_executor_claim_by_id(&tx, &claim_id)?;
            let hold = load_budget_hold_by_claim_id(&tx, &claim_id)?;
            let receipt = load_executor_settlement_receipt_by_claim_id(&tx, &claim_id)?;
            let gap = match (claim, hold, receipt) {
                (Some(claim), Some(hold), Some(receipt))
                    if claim.settlement_id.as_ref() == Some(&receipt.settlement_id) =>
                {
                    tx.execute_batch("SAVEPOINT provider_backfill;")?;
                    match post_settlement(&tx, &claim, &hold, &receipt, true) {
                        Ok(()) => {
                            tx.execute_batch("RELEASE provider_backfill;")?;
                            None
                        }
                        Err(StorageError::InvalidData(_)) | Err(StorageError::Json(_)) => {
                            tx.execute_batch(
                                "ROLLBACK TO provider_backfill; RELEASE provider_backfill;",
                            )?;
                            Some("incomplete or inconsistent provider settlement evidence")
                        }
                        Err(e) => return Err(e),
                    }
                }
                _ => Some("missing matching provider claim, receipt or hold"),
            };
            if let Some(reason) = gap {
                tx.execute(
                    "INSERT OR IGNORE INTO ledger_legacy_gaps VALUES (?1,?2)",
                    params![format!("claim:{id}"), reason],
                )?;
            }
        }
        backfill_wallet_context(&tx)?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn adjust_accounting(
        &mut self,
        command: AccountingAdjustment,
    ) -> Result<(TransactionRecord, crate::budget::state::BudgetState), StorageError> {
        if command.operation_key.trim().is_empty()
            || command.reason.trim().is_empty()
            || command.evidence.trim().is_empty()
        {
            return Err(invalid(
                "adjustment requires operation key, reason and evidence",
            ));
        }
        let cost = command.corrected_cost.scaled().map_err(ledger_error)?;
        let key = format!(
            "adjustment:{}:{:x}",
            command.owner_user_id,
            Sha256::digest(command.operation_key.as_bytes())
        );
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = ledger::find_source(&tx, &key).map_err(ledger_error)? {
            let meta = existing
                .metadata
                .as_ref()
                .ok_or_else(|| invalid("missing adjustment metadata"))?;
            if meta.original_transaction_id.as_ref() != Some(&command.original_transaction_id)
                || meta.previous_transaction_id.as_ref()
                    != Some(&command.expected_previous_transaction_id)
                || meta.effective_cost != command.corrected_cost
                || meta.reason.as_ref() != Some(&command.reason)
                || meta.adjustment_evidence.as_ref() != Some(&command.evidence)
            {
                return Err(invalid("conflicting accounting adjustment replay"));
            }
            let state = load_budget_state_from(&tx)?;
            tx.commit()?;
            return Ok((existing, state));
        }
        let original = ledger::load_transaction(&tx, &command.original_transaction_id)
            .map_err(ledger_error)?
            .ok_or_else(|| invalid("unknown original transaction"))?;
        let previous = ledger::load_transaction(&tx, &command.expected_previous_transaction_id)
            .map_err(ledger_error)?
            .ok_or_else(|| invalid("unknown previous transaction"))?;
        let original_meta = original
            .metadata
            .as_ref()
            .ok_or_else(|| invalid("legacy transaction lacks correction evidence"))?;
        let previous_meta = previous
            .metadata
            .as_ref()
            .ok_or_else(|| invalid("previous transaction lacks evidence"))?;
        if original.owner_user_id != command.owner_user_id
            || previous.owner_user_id != command.owner_user_id
            || original_meta.kind != TransactionKind::ExternalProvider
            || (previous.id != original.id
                && previous_meta.original_transaction_id.as_ref() != Some(&original.id))
            || original_meta.effective_cost.currency != command.corrected_cost.currency
        {
            return Err(invalid(
                "adjustment requires an owned external-provider original with matching currency",
            ));
        }
        let has_successor:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM ledger_transaction_metadata WHERE previous_transaction_id=?1)",[previous.id.to_string()],|r|r.get(0))?;
        if has_successor {
            return Err(invalid("adjustment predecessor is stale"));
        }
        let old = previous_meta
            .effective_cost
            .scaled()
            .map_err(ledger_error)?;
        let delta = cost
            .checked_sub(old)
            .ok_or_else(|| invalid("correction delta overflow"))?;
        let (charge, rounding) = if let Some(context) = &original_meta.context {
            let amount = command
                .corrected_cost
                .budget_cents()
                .map_err(ledger_error)?
                - previous_meta
                    .effective_cost
                    .budget_cents()
                    .map_err(ledger_error)?;
            let balance = load_budget_balance_by_id(&tx, &context.budget_id)?
                .ok_or_else(|| invalid("missing adjustment budget"))?;
            let consumed = balance
                .consumed_amount_cents
                .checked_add(amount)
                .filter(|v| *v >= 0)
                .ok_or_else(|| invalid("invalid adjusted consumption"))?;
            let remaining = balance
                .remaining_amount_cents
                .checked_sub(amount)
                .ok_or_else(|| invalid("adjustment balance overflow"))?;
            tx.execute("UPDATE budget_balances SET consumed_amount_cents=?2, remaining_amount_cents=?3, updated_at=?4 WHERE budget_id=?1",params![context.budget_id.to_string(),consumed,remaining,Utc::now().to_rfc3339()])?;
            (
                Some(amount),
                Some((i128::from(amount) * CENT_AT_SCALE_18 - delta).to_string()),
            )
        } else {
            (None, None)
        };
        let mut meta = original_meta.clone();
        meta.kind = TransactionKind::Adjustment;
        meta.source_key = key;
        meta.effective_cost = command.corrected_cost;
        meta.budget_charge_delta_cents = charge;
        meta.rounding_delta_amount = rounding;
        meta.original_transaction_id = Some(original.id.clone());
        meta.previous_transaction_id = Some(previous.id);
        meta.reason = Some(command.reason);
        meta.adjustment_evidence = Some(command.evidence);
        meta.legacy_backfill = false;
        if original.entries.len() != 2 {
            return Err(invalid(
                "correction requires an evidenced two-account expense",
            ));
        }
        let debit = original
            .entries
            .iter()
            .find(|e| e.direction == hubu_ledger::LedgerDirection::Debit)
            .ok_or_else(|| invalid("missing debit"))?;
        let credit = original
            .entries
            .iter()
            .find(|e| e.direction == hubu_ledger::LedgerDirection::Credit)
            .ok_or_else(|| invalid("missing credit"))?;
        let record = ledger::post_pair(
            &tx,
            ledger::PairPosting {
                owner: &command.owner_user_id,
                debit_kind: debit.account_kind,
                credit_kind: credit.account_kind,
                signed_amount: delta,
                metadata: meta,
                external_ref: original.external_ref,
                created_at: Utc::now(),
            },
        )
        .map_err(ledger_error)?;
        let state = load_budget_state_from(&tx)?;
        tx.commit()?;
        Ok((record, state))
    }

    pub(crate) fn query_budget_ledger(
        &self,
        query: BudgetLedgerQuery,
    ) -> Result<BudgetLedgerReport, StorageError> {
        let tx = self.conn.unchecked_transaction()?;
        // Authorization is independent of the presence of accounting rows.
        let owned:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM budgets b JOIN agent_accounts a ON a.agent_id=b.scope_id WHERE b.id=?1 AND b.scope_type='agent' AND a.agent_id=?2 AND a.owner_user_id=?3)",params![query.budget_id.to_string(),query.agent_id.to_string(),query.owner_user_id.to_string()],|r|r.get(0))?;
        if !owned {
            return Err(invalid("unknown owned agent budget"));
        }
        let all = ledger::owner_transactions(&tx, &query.owner_user_id).map_err(ledger_error)?;
        let unlinked = all
            .iter()
            .filter(|r| {
                r.metadata
                    .as_ref()
                    .and_then(|m| m.context.as_ref())
                    .is_none()
            })
            .count();
        let transactions: Vec<_> = all
            .into_iter()
            .filter(|r| {
                r.metadata
                    .as_ref()
                    .and_then(|m| m.context.as_ref())
                    .is_some_and(|c| c.agent_id == query.agent_id && c.budget_id == query.budget_id)
            })
            .collect();
        let total: i128 = transactions
            .iter()
            .filter_map(|r| r.metadata.as_ref()?.budget_charge_delta_cents)
            .map(i128::from)
            .sum();
        let total = i64::try_from(total).map_err(|_| invalid("charge total overflow"))?;
        let balance = load_budget_balance_by_id(&tx, &query.budget_id)?
            .ok_or_else(|| invalid("missing accounting budget"))?;
        let pending:i64=tx.query_row("SELECT COUNT(*) FROM budget_holds WHERE budget_id=?1 AND status IN ('frozen','claimed')",[query.budget_id.to_string()],|r|r.get(0))?;
        Ok(BudgetLedgerReport {
            budget_id: query.budget_id,
            agent_id: query.agent_id,
            transactions,
            consumed_amount_cents: balance.consumed_amount_cents,
            recorded_budget_charges_cents: total,
            unaccounted_consumption_cents: balance
                .consumed_amount_cents
                .checked_sub(total)
                .ok_or_else(|| invalid("coverage difference overflow"))?,
            owner_transactions_without_budget_context: unlinked,
            pending_hold_count: pending,
        })
    }
}

fn backfill_wallet_context(tx: &rusqlite::Transaction<'_>) -> Result<(), StorageError> {
    if !table_has_column(tx, "payment_attempts", "ledger_transaction_id")? {
        return Ok(());
    }
    let ids = {
        let mut stmt=tx.prepare("SELECT t.id FROM ledger_transactions t LEFT JOIN ledger_transaction_metadata m ON m.transaction_id=t.id WHERE t.kind='wallet_payment' AND m.transaction_id IS NULL")?;
        let ids = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids
    };
    for id in ids {
        let transaction_id: LedgerTransactionId = id
            .parse()
            .map_err(|_| invalid("invalid legacy wallet transaction id"))?;
        let record = ledger::load_transaction(tx, &transaction_id)
            .map_err(ledger_error)?
            .ok_or_else(|| invalid("missing wallet transaction"))?;
        let matches = {
            let mut stmt=tx.prepare("SELECT p.payment_id,p.spend_auth_token_id,p.agent_account_id,d.request_json,d.operation_key,d.id,h.budget_id,h.budget_version_id,p.amount_cents,p.rail_reference,d.agent_id
            FROM payment_attempts p JOIN spend_auth_tokens t ON t.id=p.spend_auth_token_id
            JOIN spend_decisions d ON d.id=t.spend_decision_id JOIN budget_holds h ON h.spend_decision_id=d.id
            JOIN budgets b ON b.id=h.budget_id AND b.scope_type='agent' AND b.scope_id=d.agent_id
            WHERE p.ledger_transaction_id=?1 AND p.owner_user_id=?2 AND p.status='succeeded'
            AND p.payment_id=?3 AND t.used_by_payment_id=p.payment_id AND t.owner_user_id=p.owner_user_id
            AND d.owner_user_id=p.owner_user_id AND d.agent_id=p.agent_id AND h.amount_cents=p.amount_cents AND h.currency=p.currency AND h.status='settled' AND h.executor_claim_id IS NULL")?;
            let rows = stmt
                .query_map(
                    params![id, record.owner_user_id.to_string(), record.external_ref],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, String>(4)?,
                            r.get::<_, String>(5)?,
                            r.get::<_, String>(6)?,
                            r.get::<_, String>(7)?,
                            r.get::<_, i64>(8)?,
                            r.get::<_, Option<String>>(9)?,
                            r.get::<_, String>(10)?,
                        ))
                    },
                )?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        if matches.len() != 1 {
            tx.execute("INSERT OR IGNORE INTO ledger_legacy_gaps VALUES (?1,'wallet transaction lacks unique consistent governed-spend evidence')",[format!("wallet:{id}")])?;
            continue;
        }
        let (
            payment_id,
            token,
            account,
            json,
            operation,
            decision,
            budget,
            version,
            amount,
            rail_reference,
            decision_agent_id,
        ) = &matches[0];
        let request: SpendRequest = serde_json::from_str(json)?;
        let expected = ExactMoney {
            amount: amount.to_string(),
            scale: 2,
            currency: request.currency,
        };
        let debits: Vec<_> = record
            .entries
            .iter()
            .filter(|e| e.direction == hubu_ledger::LedgerDirection::Debit)
            .collect();
        let credits: Vec<_> = record
            .entries
            .iter()
            .filter(|e| e.direction == hubu_ledger::LedgerDirection::Credit)
            .collect();
        let accounts_match: bool = tx.query_row("SELECT NOT EXISTS(SELECT 1 FROM ledger_entries e JOIN ledger_accounts a ON a.id=e.account_id WHERE e.transaction_id=?1 AND (e.owner_user_id!=?2 OR a.owner_user_id!=?2 OR e.currency!=?3 OR a.currency!=?3))", params![id,record.owner_user_id.to_string(),request.currency.to_string()], |r|r.get(0))?;
        if request.owner_user_id != record.owner_user_id
            || request.agent_id.to_string() != *decision_agent_id
            || request.agent_account_id.to_string() != *account
            || request.amount_cents != *amount
            || record.entries.len() != 2
            || debits.len() != 1
            || credits.len() != 1
            || !accounts_match
            || debits[0].account_kind != LedgerAccountKind::AgentSpendExpense
            || credits[0].account_kind != LedgerAccountKind::UserWalletCash
            || credits[0].amount.scaled().map_err(ledger_error)?
                != expected.scaled().map_err(ledger_error)?
            || debits[0].amount.currency != request.currency
            || credits[0].amount.currency != request.currency
            || debits[0].amount.scaled().map_err(ledger_error)?
                != expected.scaled().map_err(ledger_error)?
        {
            tx.execute("INSERT OR IGNORE INTO ledger_legacy_gaps VALUES (?1,'wallet amount or account evidence is inconsistent')",[format!("wallet:{id}")])?;
            continue;
        }
        let meta = TransactionMetadata {
            kind: TransactionKind::WalletPayment,
            source_key: format!("payment:{payment_id}"),
            agent_id: Some(request.agent_id.clone()),
            agent_account_id: Some(request.agent_account_id.clone()),
            context: Some(GovernedSpendContext {
                agent_id: request.agent_id,
                agent_account_id: request.agent_account_id,
                budget_id: budget.parse().map_err(|_| invalid("invalid budget"))?,
                budget_version_id: version
                    .parse()
                    .map_err(|_| invalid("invalid budget version"))?,
                spend_decision_id: decision.parse().map_err(|_| invalid("invalid decision"))?,
                operation_key: operation.clone(),
            }),
            effective_cost: expected,
            budget_charge_delta_cents: Some(*amount),
            rounding_delta_amount: Some("0".into()),
            original_transaction_id: None,
            previous_transaction_id: None,
            provider: None,
            billing_merchant: request.merchant,
            purpose: (!request.reason.is_empty()).then_some(request.reason),
            source_evidence: serde_json::json!({"payment_id":payment_id,"spend_auth_token_id":token,"rail_reference":rail_reference}),
            reason: None,
            adjustment_evidence: None,
            legacy_backfill: true,
            missing_evidence: vec![],
        };
        ledger::attach_metadata(tx, &transaction_id, &meta).map_err(ledger_error)?;
    }
    Ok(())
}
