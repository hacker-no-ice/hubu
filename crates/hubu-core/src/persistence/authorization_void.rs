//! Cancel an admission-rejected authorization without inventing a claim or charge.
use super::*;
use hubu_common::ids::{AgentAccountId, SpendAuthTokenId};

pub struct AuthorizationVoidResult {
    pub token: SpendAuthTokenRecord,
    pub hold: BudgetHold,
    pub balance: BudgetBalance,
    pub idempotent_replay: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthorizationVoidError {
    #[error("authorization is claimed or used")]
    ClaimedOrUsed,
    #[error("authorization is revoked or no longer frozen")]
    RevokedOrFinalized,
    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl From<rusqlite::Error> for AuthorizationVoidError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.into())
    }
}

impl SqliteGovernanceRepository {
    pub fn void_unclaimed_authorization(
        &mut self,
        owner: &UserId,
        account: &AgentAccountId,
        token_id: &SpendAuthTokenId,
        operation_key: &str,
    ) -> Result<AuthorizationVoidResult, AuthorizationVoidError> {
        let invalid = |message: &str| StorageError::InvalidData(message.into());
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut token = load_spend_auth_token_by_id(&tx, token_id)?
            .ok_or_else(|| invalid("unknown owned authorization"))?;
        let bound: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM spend_decisions d JOIN agent_accounts a ON a.agent_id=d.agent_id
             WHERE d.id=?1 AND d.owner_user_id=?2 AND a.owner_user_id=?2 AND a.id=?3 AND d.operation_key=?4)",
            params![token.spend_decision_id.to_string(), owner.to_string(), account.to_string(), operation_key],
            |row| row.get(0))?;
        if token.owner_user_id != *owner || !bound || operation_key.trim().is_empty() {
            return Err(invalid("unknown owned authorization operation").into());
        }
        let mut hold = tx.query_row(
            "SELECT id,budget_id,budget_version_id,spend_decision_id,amount,currency,status,executor_claim_id,created_at,updated_at,expires_at,task_budget_id
             FROM budget_holds WHERE spend_decision_id=?1",
            [token.spend_decision_id.to_string()], budget_hold_from_row)?;
        let mut balance = load_budget_balance_by_id(&tx, &hold.budget_id)?
            .ok_or_else(|| invalid("authorization budget balance is missing"))?;
        let owned_budget: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM budgets b JOIN agent_accounts a ON a.agent_id=b.scope_id
             WHERE b.id=?1 AND b.scope_type='agent' AND a.id=?2 AND a.owner_user_id=?3)",
            params![
                hold.budget_id.to_string(),
                account.to_string(),
                owner.to_string()
            ],
            |row| row.get(0),
        )?;
        if !owned_budget {
            return Err(invalid("authorization hold belongs to another agent").into());
        }
        let claimed: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM spend_executor_claims WHERE spend_auth_token_id=?1)",
            [token_id.to_string()],
            |row| row.get(0),
        )?;
        if claimed
            || hold.executor_claim_id.is_some()
            || token.used_at.is_some()
            || token.used_by_payment_id.is_some()
        {
            return Err(AuthorizationVoidError::ClaimedOrUsed);
        }
        let voided: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM spend_authorization_voids WHERE spend_auth_token_id=?1 AND reason='executor_rejected_before_claim')",
            [token_id.to_string()], |row| row.get(0))?;
        if voided && token.revoked_at.is_some() && matches!(hold.status, BudgetHoldStatus::Released)
        {
            tx.commit()?;
            return Ok(AuthorizationVoidResult {
                token,
                hold,
                balance,
                idempotent_replay: true,
            });
        }
        if token.revoked_at.is_some() || !matches!(hold.status, BudgetHoldStatus::Frozen) {
            return Err(AuthorizationVoidError::RevokedOrFinalized);
        }
        validate_hold_and_balance_relationship(&tx, &hold, &balance, false)?;
        balance = checked_executor_finalization_balance(&balance, &hold, None)?;
        let now = Utc::now();
        token.revoked_at = Some(now);
        hold.status = BudgetHoldStatus::Released;
        hold.updated_at = now;
        tx.execute(
            "UPDATE spend_auth_tokens SET revoked_at=?2 WHERE id=?1",
            params![token_id.to_string(), now.to_rfc3339()],
        )?;
        tx.execute(
            "UPDATE budget_holds SET status='released',updated_at=?2 WHERE id=?1",
            params![hold.id.to_string(), now.to_rfc3339()],
        )?;
        upsert_balance(&tx, &balance)?;
        tx.execute("INSERT INTO spend_authorization_voids(spend_auth_token_id,reason,voided_at) VALUES(?1,'executor_rejected_before_claim',?2)",
            params![token_id.to_string(), now.to_rfc3339()])?;
        super::financial_lifecycle::capture_authorization_release(
            &tx,
            &token.spend_decision_id,
            crate::spend::lifecycle::AuthorizationReleaseReason::ExecutorRejectedBeforeClaim,
            now,
        )?;
        tx.commit()?;
        Ok(AuthorizationVoidResult {
            token,
            hold,
            balance,
            idempotent_replay: false,
        })
    }
}
