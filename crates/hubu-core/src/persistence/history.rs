//! Read-only, consistent owner snapshot for public history projections.
use super::*;
use hubu_ledger::TransactionRecord;

#[derive(Debug)]
pub struct HistorySnapshot {
    pub authorization_outcomes:
        std::collections::HashMap<hubu_common::ids::SpendDecisionId, SpendAuthorizationDecision>,
    pub owned_agent_accounts: std::collections::HashMap<AgentId, hubu_common::ids::AgentAccountId>,
    pub transactions: Vec<TransactionRecord>,
    pub decisions: Vec<SpendDecisionRecord>,
    pub tokens: Vec<SpendAuthTokenRecord>,
    pub claims: Vec<SpendExecutorClaimRecord>,
    pub receipts: Vec<PersistedSpendExecutorSettlementReceipt>,
    pub budgets: Vec<Budget>,
    pub balances: Vec<BudgetBalance>,
    pub holds: Vec<BudgetHold>,
}

impl SqliteGovernanceRepository {
    /// All linked balances, receipts and postings come from one SQLite read
    /// transaction. Callers paginate the immutable identities after projecting
    /// safe public fields; never derive budget coverage from just a page.
    pub fn history_snapshot(&self, owner: &UserId) -> Result<HistorySnapshot, StorageError> {
        let tx = self.conn.unchecked_transaction()?;
        let transactions = hubu_ledger::domain::owner_transactions(&tx, owner)
            .map_err(|e| StorageError::InvalidData(e.to_string()))?;
        let decisions = {
            let mut stmt=tx.prepare("SELECT id, owner_user_id, agent_id, operation_key, revision, actor, request_json, evaluation_json, created_at FROM spend_decisions WHERE owner_user_id=?1")?;
            let rows = stmt.query_map([owner.to_string()], |row| {
                let mut request: SpendRequest = parse_json(&row.get::<_, String>(6)?)?;
                request.normalize_legacy_reason();
                let agent_id: String = row.get(2)?;
                if request.agent_id.to_string() != agent_id || request.owner_user_id != *owner {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                Ok(SpendDecisionRecord {
                    id: parse_id(&row.get::<_, String>(0)?)?,
                    owner_user_id: parse_id(&row.get::<_, String>(1)?)?,
                    operation_key: row.get(3)?,
                    revision: row.get(4)?,
                    actor: row.get(5)?,
                    request,
                    evaluation: parse_json(&row.get::<_, String>(7)?)?,
                    created_at: parse_timestamp(&row.get::<_, String>(8)?)?,
                })
            })?;
            collect_rows(rows)?
        };
        let authorization_outcomes = {
            let mut stmt=tx.prepare("SELECT d.id,o.decision FROM spend_decisions d JOIN spend_authorization_outcomes o ON o.id=(SELECT MAX(latest.id) FROM spend_authorization_outcomes latest WHERE latest.agent_id=d.agent_id AND latest.operation_key=d.operation_key AND latest.revision=d.revision) WHERE d.owner_user_id=?1")?;
            let rows = stmt
                .query_map([owner.to_string()], |row| {
                    Ok((
                        parse_id(&row.get::<_, String>(0)?)?,
                        parse_spend_authorization_decision(&row.get::<_, String>(1)?)?,
                    ))
                })?
                .collect::<Result<std::collections::HashMap<_, _>, _>>()?;
            rows
        };
        let tokens = {
            let mut stmt=tx.prepare("SELECT id, owner_user_id, spend_decision_id, expires_at, claim_ttl_seconds, used_at, used_by_payment_id, revoked_at FROM spend_auth_tokens WHERE owner_user_id=?1")?;
            let rows = stmt.query_map([owner.to_string()], spend_auth_token_from_row)?;
            collect_rows(rows)?
        };
        let claims = {
            let mut stmt=tx.prepare("SELECT id, spend_auth_token_id, owner_user_id, agent_id, operation_key, lease_profile, status, claimed_at, expires_at, finalized_at, settlement_id, provider_reference, reconciliation_evidence, reconciled_at, reconciled_by_user_id FROM spend_executor_claims WHERE owner_user_id=?1")?;
            let rows = stmt.query_map([owner.to_string()], executor_claim_from_row)?;
            collect_rows(rows)?
        };
        let receipts = claims
            .iter()
            .map(|c| load_executor_settlement_receipt_by_claim_id(&tx, &c.id))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect();
        let owned_agent_accounts = {
            let mut stmt =
                tx.prepare("SELECT agent_id,id FROM agent_accounts WHERE owner_user_id=?1")?;
            let rows=stmt.query_map([owner.to_string()], |row| Ok((parse_id(&row.get::<_,String>(0)?)?,parse_id(&row.get::<_,String>(1)?)?)))?
                .collect::<Result<std::collections::HashMap<AgentId,hubu_common::ids::AgentAccountId>,_>>()?;
            rows
        };
        let budgets: Vec<_> = load_budgets_from(&tx)?
            .into_iter()
            .filter(|b| owned_agent_accounts.contains_key(&b.agent_id))
            .collect();
        let budget_ids: std::collections::HashSet<_> =
            budgets.iter().map(|b| b.id.clone()).collect();
        let balances = load_budget_balances_from(&tx)?
            .into_iter()
            .filter(|b| budget_ids.contains(&b.budget_id))
            .collect();
        let holds = load_budget_holds_from(&tx)?
            .into_iter()
            .filter(|h| budget_ids.contains(&h.budget_id))
            .collect();
        Ok(HistorySnapshot {
            authorization_outcomes,
            owned_agent_accounts,
            transactions,
            decisions,
            tokens,
            claims,
            receipts,
            budgets,
            balances,
            holds,
        })
    }
}
