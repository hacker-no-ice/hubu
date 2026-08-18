use std::path::Path;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use hubu_common::ids::{AgentId, PaymentId, PolicyId, SpendExecutorClaimId, UserId};
use hubu_common::money::Currency;
use hubu_common::time::TimePeriod;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::budget::{Budget, BudgetBalance, BudgetHold, BudgetHoldStatus, BudgetStatus};
use crate::policy::Policy;
use crate::spend::{
    PersistedSpendExecutorSettlementReceipt, SpendAttemptAuditRecord, SpendAuthTokenRecord,
    SpendAuthorizationDecision, SpendDecisionRecord, SpendExecutorClaimRecord,
    SpendExecutorClaimStatus, SpendExecutorSettlementReceipt, SpendRequest,
};
use crate::spending_target::{SpendingTarget, SpendingTargetStatus};
use crate::storage::StorageError;

pub trait PolicyRepository {
    fn save_policy_assignment(
        &mut self,
        owner_user_id: &UserId,
        scope: &PolicyAssignmentScope,
        policy: &Policy,
    ) -> Result<(), StorageError> {
        self.apply_policy(
            owner_user_id,
            &policy.id,
            &policy.id,
            scope,
            policy,
            None,
            None,
            "system:compatibility",
            "legacy_save_policy_assignment",
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_policy(
        &mut self,
        owner_user_id: &UserId,
        declarative_key: &str,
        display_name: &str,
        scope: &PolicyAssignmentScope,
        policy: &Policy,
        expected_revision: Option<u64>,
        expected_hash: Option<&str>,
        actor: &str,
        source: &str,
    ) -> Result<PolicyApplyResult, StorageError>;

    fn load_policy_assignments(&self) -> Result<Vec<PolicyAssignmentRecord>, StorageError>;
    fn load_policies(&self, owner_user_id: &UserId) -> Result<Vec<PolicyResource>, StorageError>;
    fn load_policy(
        &self,
        owner_user_id: &UserId,
        selector: &str,
    ) -> Result<Option<PolicyResource>, StorageError>;
    fn load_policy_history(
        &self,
        owner_user_id: &UserId,
        selector: &str,
    ) -> Result<Vec<PolicyRevisionRecord>, StorageError>;
    fn load_policy_revision(
        &self,
        owner_user_id: &UserId,
        selector: &str,
        revision: u64,
    ) -> Result<Option<PolicyRevisionRecord>, StorageError>;
    fn load_policy_audit(
        &self,
        owner_user_id: &UserId,
        selector: &str,
    ) -> Result<Vec<PolicyAuditRecord>, StorageError>;
}

pub trait SpendRepository {
    fn admit_spend_attempt(
        &mut self,
        owner_user_id: &UserId,
        operation_key: &str,
        request: &SpendRequest,
        actor: &str,
        submitted_at: DateTime<Utc>,
    ) -> Result<SpendAttemptAdmission, StorageError>;
    fn record_spend_attempt_outcome(
        &mut self,
        record: &SpendDecisionRecord,
        decision: SpendAuthorizationDecision,
        reasons: &[String],
        decided_at: DateTime<Utc>,
    ) -> Result<(), StorageError>;
    fn load_spend_attempt_history(
        &self,
        agent_id: &AgentId,
        operation_key: &str,
    ) -> Result<Vec<SpendAttemptAuditRecord>, StorageError>;
    fn changed_scope_retry_is_safe(
        &self,
        agent_id: &AgentId,
        operation_key: &str,
    ) -> Result<bool, StorageError>;
    fn save_spend_decision(&mut self, record: &SpendDecisionRecord) -> Result<(), StorageError>;
    fn save_spend_auth_token(&mut self, record: &SpendAuthTokenRecord) -> Result<(), StorageError>;
    fn update_spend_auth_token(
        &mut self,
        record: &SpendAuthTokenRecord,
    ) -> Result<(), StorageError>;
    fn load_spend_decisions(&self) -> Result<Vec<SpendDecisionRecord>, StorageError>;
    fn load_spend_auth_tokens(&self) -> Result<Vec<SpendAuthTokenRecord>, StorageError>;
    fn save_executor_claim(
        &mut self,
        record: &SpendExecutorClaimRecord,
    ) -> Result<(), StorageError>;
    fn load_executor_claims(&self) -> Result<Vec<SpendExecutorClaimRecord>, StorageError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpendAttemptAdmission {
    Admitted { revision: u64 },
    ExactReplay { revision: u64 },
    ChangedScopeBlocked,
}

pub trait BudgetRepository {
    fn expire_overdue_budget_holds(&mut self, now: DateTime<Utc>) -> Result<(), StorageError>;
    fn save_budget_with_balance(
        &mut self,
        budget: &Budget,
        balance: &BudgetBalance,
    ) -> Result<(), StorageError>;
    fn save_budget_hold(
        &mut self,
        hold: &BudgetHold,
        balance: &BudgetBalance,
    ) -> Result<(), StorageError>;
    fn update_budget_hold(
        &mut self,
        hold: &BudgetHold,
        balance: &BudgetBalance,
    ) -> Result<(), StorageError>;
    fn load_budgets(&self) -> Result<Vec<Budget>, StorageError>;
    fn load_budget_balances(&self) -> Result<Vec<BudgetBalance>, StorageError>;
    fn load_budget_holds(&self) -> Result<Vec<BudgetHold>, StorageError>;
}

pub trait SpendingTargetRepository {
    fn save_spending_target(&mut self, target: &SpendingTarget) -> Result<(), StorageError>;
    fn load_spending_targets(&self) -> Result<Vec<SpendingTarget>, StorageError>;
}

#[derive(Debug, Clone)]
pub struct PolicyAssignmentRecord {
    pub owner_user_id: UserId,
    pub scope: PolicyAssignmentScope,
    pub policy_id: String,
    pub policy: Policy,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PolicyResource {
    pub policy_id: String,
    pub owner_user_id: UserId,
    pub declarative_key: String,
    pub display_name: String,
    pub current_revision: u64,
    pub current_hash: String,
    pub policy: Policy,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PolicyRevisionRecord {
    pub policy_id: String,
    pub revision: u64,
    pub payload_hash: String,
    pub policy: Policy,
    pub actor: String,
    pub source: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PolicyAuditRecord {
    pub policy_id: String,
    pub actor: String,
    pub source: String,
    pub occurred_at: DateTime<Utc>,
    pub old_hash: Option<String>,
    pub new_hash: String,
    pub affected_assignments: Vec<String>,
    pub action: String,
}

#[derive(Debug, Clone)]
pub struct PolicyApplyResult {
    pub resource: PolicyResource,
    pub assignment: PolicyAssignmentRecord,
    pub changed: bool,
    pub assignment_changed: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PolicyAuditAssignments(Vec<String>);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PolicyAssignmentScope {
    UserDefault,
    AgentOverride(AgentId),
}

impl PolicyAssignmentScope {
    pub fn scope_type(&self) -> &'static str {
        match self {
            Self::UserDefault => "user_default",
            Self::AgentOverride(_) => "agent_override",
        }
    }

    pub fn scope_id(&self) -> String {
        match self {
            Self::UserDefault => "default".to_string(),
            Self::AgentOverride(agent_id) => agent_id.to_string(),
        }
    }

    fn agent_id(&self) -> Option<String> {
        match self {
            Self::UserDefault => None,
            Self::AgentOverride(agent_id) => Some(agent_id.to_string()),
        }
    }

    fn from_parts(scope_type: &str, scope_id: &str) -> Result<Self, StorageError> {
        match scope_type {
            "user_default" => Ok(Self::UserDefault),
            "agent_override" => AgentId::from_str(scope_id)
                .map(Self::AgentOverride)
                .map_err(|_| StorageError::InvalidData(format!("invalid agent id `{scope_id}`"))),
            other => Err(StorageError::InvalidData(format!(
                "unknown policy assignment scope `{other}`"
            ))),
        }
    }
}

pub struct SqliteGovernanceRepository {
    conn: Connection,
}

#[derive(Debug, Clone)]
pub struct ExecutorFinalizationResult {
    pub claim: SpendExecutorClaimRecord,
    pub token: SpendAuthTokenRecord,
    pub hold: BudgetHold,
    pub balance: BudgetBalance,
    pub receipt: Option<PersistedSpendExecutorSettlementReceipt>,
    pub idempotent_replay: bool,
}

/// Durable boundary used by the claim application service.
///
/// Implementations must commit each claim/hold transition atomically. The
/// service owns workflow orchestration while the repository remains the
/// concurrency authority for terminal claim state.
pub trait ExecutorClaimRepository {
    fn save_executor_claim_with_budget_hold(
        &mut self,
        claim: &SpendExecutorClaimRecord,
        hold: &BudgetHold,
        balance: &BudgetBalance,
    ) -> Result<(), StorageError>;

    fn settle_executor_claim_transactionally(
        &mut self,
        owner_user_id: &UserId,
        agent_id: &AgentId,
        operation_key: &str,
        proposed_settlement_id: PaymentId,
        receipt: SpendExecutorSettlementReceipt,
        settlement_started_at: DateTime<Utc>,
    ) -> Result<ExecutorFinalizationResult, StorageError>;

    fn release_executor_claim_transactionally(
        &mut self,
        owner_user_id: &UserId,
        agent_id: &AgentId,
        operation_key: &str,
        finalization_started_at: DateTime<Utc>,
    ) -> Result<ExecutorFinalizationResult, StorageError>;

    // These fields form one auditable atomic reconciliation command at the storage boundary.
    #[allow(clippy::too_many_arguments)]
    fn reconcile_executor_claim_as_billed_transactionally(
        &mut self,
        claim_id: &SpendExecutorClaimId,
        owner_user_id: &UserId,
        provider_reference: &str,
        evidence: &str,
        proposed_settlement_id: PaymentId,
        receipt: SpendExecutorSettlementReceipt,
        reconciliation_started_at: DateTime<Utc>,
    ) -> Result<ExecutorFinalizationResult, StorageError>;

    fn reconcile_executor_claim_as_not_billed_transactionally(
        &mut self,
        claim_id: &SpendExecutorClaimId,
        owner_user_id: &UserId,
        provider_reference: &str,
        evidence: &str,
        reconciliation_started_at: DateTime<Utc>,
    ) -> Result<ExecutorFinalizationResult, StorageError>;
}

enum ExecutorFinalizationAction {
    Settle {
        settlement_id: PaymentId,
        receipt: SpendExecutorSettlementReceipt,
    },
    Release,
}

#[derive(Clone, Copy)]
enum ExecutorClaimLocator<'a> {
    Operation {
        agent_id: &'a AgentId,
        operation_key: &'a str,
    },
    ClaimId(&'a SpendExecutorClaimId),
}

enum ExecutorFinalizationAuthority {
    Executor,
    Reconciliation {
        provider_reference: String,
        evidence: String,
        reconciled_by_user_id: UserId,
    },
}

impl SqliteGovernanceRepository {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self, StorageError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    pub fn save_executor_claim_with_budget_hold(
        &mut self,
        claim: &SpendExecutorClaimRecord,
        hold: &BudgetHold,
        balance: &BudgetBalance,
    ) -> Result<(), StorageError> {
        let sqlite_tx = self.conn.transaction()?;
        sqlite_tx.execute(
            "INSERT INTO spend_executor_claims
             (id, spend_auth_token_id, owner_user_id, agent_id, operation_key,
              workload_profile, status, claimed_at, expires_at, finalized_at, settlement_id,
              provider_reference, reconciliation_evidence, reconciled_at, reconciled_by_user_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(id) DO NOTHING",
            params![
                claim.id.to_string(),
                claim.spend_auth_token_id.to_string(),
                claim.owner_user_id.to_string(),
                claim.agent_id.to_string(),
                claim.operation_key,
                claim.workload_profile,
                executor_claim_status(&claim.status),
                claim.claimed_at.to_rfc3339(),
                claim.expires_at.to_rfc3339(),
                claim.finalized_at.map(|timestamp| timestamp.to_rfc3339()),
                claim.settlement_id.as_ref().map(ToString::to_string),
                claim.provider_reference,
                claim.reconciliation_evidence,
                claim.reconciled_at.map(|timestamp| timestamp.to_rfc3339()),
                claim
                    .reconciled_by_user_id
                    .as_ref()
                    .map(ToString::to_string),
            ],
        )?;
        sqlite_tx.execute(
            "UPDATE budget_holds
             SET status = ?2, executor_claim_id = ?3, updated_at = ?4, expires_at = ?5
             WHERE id = ?1",
            params![
                hold.id.to_string(),
                budget_hold_status(&hold.status),
                hold.executor_claim_id.as_ref().map(ToString::to_string),
                hold.updated_at.to_rfc3339(),
                hold.expires_at.to_rfc3339(),
            ],
        )?;
        upsert_balance(&sqlite_tx, balance)?;
        refresh_persisted_budget_status(&sqlite_tx, &hold.budget_id.to_string(), hold.updated_at)?;
        sqlite_tx.commit()?;
        Ok(())
    }

    pub fn settle_executor_claim_transactionally(
        &mut self,
        owner_user_id: &UserId,
        agent_id: &AgentId,
        operation_key: &str,
        proposed_settlement_id: PaymentId,
        receipt: SpendExecutorSettlementReceipt,
        settlement_started_at: DateTime<Utc>,
    ) -> Result<ExecutorFinalizationResult, StorageError> {
        self.finalize_executor_claim_transactionally(
            owner_user_id,
            ExecutorClaimLocator::Operation {
                agent_id,
                operation_key,
            },
            ExecutorFinalizationAction::Settle {
                settlement_id: proposed_settlement_id,
                receipt,
            },
            ExecutorFinalizationAuthority::Executor,
            settlement_started_at,
        )
    }

    pub fn release_executor_claim_transactionally(
        &mut self,
        owner_user_id: &UserId,
        agent_id: &AgentId,
        operation_key: &str,
        finalization_started_at: DateTime<Utc>,
    ) -> Result<ExecutorFinalizationResult, StorageError> {
        self.finalize_executor_claim_transactionally(
            owner_user_id,
            ExecutorClaimLocator::Operation {
                agent_id,
                operation_key,
            },
            ExecutorFinalizationAction::Release,
            ExecutorFinalizationAuthority::Executor,
            finalization_started_at,
        )
    }

    // Keep the full reconciliation evidence explicit at this transactional boundary.
    #[allow(clippy::too_many_arguments)]
    pub fn reconcile_executor_claim_as_billed_transactionally(
        &mut self,
        claim_id: &SpendExecutorClaimId,
        owner_user_id: &UserId,
        provider_reference: &str,
        evidence: &str,
        proposed_settlement_id: PaymentId,
        receipt: SpendExecutorSettlementReceipt,
        reconciliation_started_at: DateTime<Utc>,
    ) -> Result<ExecutorFinalizationResult, StorageError> {
        self.reconcile_executor_claim_transactionally(
            claim_id,
            owner_user_id,
            provider_reference,
            evidence,
            ExecutorFinalizationAction::Settle {
                settlement_id: proposed_settlement_id,
                receipt,
            },
            reconciliation_started_at,
        )
    }

    pub fn reconcile_executor_claim_as_not_billed_transactionally(
        &mut self,
        claim_id: &SpendExecutorClaimId,
        owner_user_id: &UserId,
        provider_reference: &str,
        evidence: &str,
        reconciliation_started_at: DateTime<Utc>,
    ) -> Result<ExecutorFinalizationResult, StorageError> {
        self.reconcile_executor_claim_transactionally(
            claim_id,
            owner_user_id,
            provider_reference,
            evidence,
            ExecutorFinalizationAction::Release,
            reconciliation_started_at,
        )
    }

    fn reconcile_executor_claim_transactionally(
        &mut self,
        claim_id: &SpendExecutorClaimId,
        owner_user_id: &UserId,
        provider_reference: &str,
        evidence: &str,
        action: ExecutorFinalizationAction,
        reconciliation_started_at: DateTime<Utc>,
    ) -> Result<ExecutorFinalizationResult, StorageError> {
        let provider_reference = provider_reference.trim();
        if provider_reference.is_empty() {
            return Err(StorageError::InvalidData(
                "provider reference cannot be empty".to_string(),
            ));
        }
        let evidence = evidence.trim();
        if evidence.is_empty() {
            return Err(StorageError::InvalidData(
                "reconciliation evidence cannot be empty".to_string(),
            ));
        }
        self.finalize_executor_claim_transactionally(
            owner_user_id,
            ExecutorClaimLocator::ClaimId(claim_id),
            action,
            ExecutorFinalizationAuthority::Reconciliation {
                provider_reference: provider_reference.to_string(),
                evidence: evidence.to_string(),
                reconciled_by_user_id: owner_user_id.clone(),
            },
            reconciliation_started_at,
        )
    }

    fn finalize_executor_claim_transactionally(
        &mut self,
        owner_user_id: &UserId,
        locator: ExecutorClaimLocator<'_>,
        action: ExecutorFinalizationAction,
        authority: ExecutorFinalizationAuthority,
        finalization_started_at: DateTime<Utc>,
    ) -> Result<ExecutorFinalizationResult, StorageError> {
        let sqlite_tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut claim = match locator {
            ExecutorClaimLocator::Operation {
                agent_id,
                operation_key,
            } => load_executor_claim_by_operation(&sqlite_tx, agent_id, operation_key)?,
            ExecutorClaimLocator::ClaimId(claim_id) => {
                load_executor_claim_by_id(&sqlite_tx, claim_id)?
            }
        }
        .ok_or_else(|| StorageError::InvalidData("unknown executor claim".to_string()))?;
        if &claim.owner_user_id != owner_user_id {
            return Err(StorageError::InvalidData(
                "executor claim owner does not match".to_string(),
            ));
        }
        if let ExecutorClaimLocator::Operation {
            agent_id,
            operation_key,
        } = locator
        {
            if claim.operation_key != operation_key {
                return Err(StorageError::InvalidData(
                    "executor claim operation key does not match".to_string(),
                ));
            }
            if &claim.agent_id != agent_id {
                return Err(StorageError::InvalidData(
                    "executor claim agent does not match".to_string(),
                ));
            }
        }

        let mut token = load_spend_auth_token_by_id(&sqlite_tx, &claim.spend_auth_token_id)?
            .ok_or_else(|| {
                StorageError::InvalidData("executor claim token is missing".to_string())
            })?;
        let mut hold = load_budget_hold_by_claim_id(&sqlite_tx, &claim.id)?.ok_or_else(|| {
            StorageError::InvalidData("executor claim budget hold is missing".to_string())
        })?;
        let mut balance =
            load_budget_balance_by_id(&sqlite_tx, &hold.budget_id)?.ok_or_else(|| {
                StorageError::InvalidData("executor claim budget balance is missing".to_string())
            })?;

        let mut persisted_receipt =
            load_executor_settlement_receipt_by_claim_id(&sqlite_tx, &claim.id)?;
        let replay_is_consistent = match (&action, &claim.status) {
            (
                ExecutorFinalizationAction::Settle { receipt, .. },
                SpendExecutorClaimStatus::Settled,
            ) => {
                let settlement_id = claim.settlement_id.as_ref().ok_or_else(|| {
                    StorageError::InvalidData(
                        "settled executor claim has no settlement id".to_string(),
                    )
                })?;
                let stored_receipt = persisted_receipt.as_ref().ok_or_else(|| {
                    StorageError::InvalidData(
                        "settled executor claim has no settlement receipt".to_string(),
                    )
                })?;
                if &stored_receipt.receipt != receipt {
                    return Err(StorageError::InvalidData(
                        "settlement receipt does not match the original settlement".to_string(),
                    ));
                }
                token.used_by_payment_id.as_ref() == Some(settlement_id)
                    && token.used_at.is_some()
                    && token.revoked_at.is_none()
                    && matches!(hold.status, BudgetHoldStatus::Settled)
                    && hold.executor_claim_id.as_ref() == Some(&claim.id)
                    && &stored_receipt.settlement_id == settlement_id
                    && stored_receipt.authorized_max_cents == hold.amount_cents
                    && stored_receipt.receipt.actual_vendor_cost_cents >= 0
                    && stored_receipt.receipt.actual_vendor_cost_cents <= hold.amount_cents
                    && stored_receipt.released_amount_cents
                        == hold.amount_cents - stored_receipt.receipt.actual_vendor_cost_cents
                    && stored_receipt.currency == hold.currency
            }
            (ExecutorFinalizationAction::Release, SpendExecutorClaimStatus::Released) => {
                token.revoked_at.is_some()
                    && token.used_at.is_none()
                    && token.used_by_payment_id.is_none()
                    && matches!(hold.status, BudgetHoldStatus::Released)
                    && hold.executor_claim_id.as_ref() == Some(&claim.id)
                    && persisted_receipt.is_none()
            }
            (ExecutorFinalizationAction::Settle { .. }, SpendExecutorClaimStatus::Released) => {
                return Err(StorageError::InvalidData(
                    "executor claim has already been released".to_string(),
                ));
            }
            (ExecutorFinalizationAction::Release, SpendExecutorClaimStatus::Settled) => {
                return Err(StorageError::InvalidData(
                    "executor claim has already been settled".to_string(),
                ));
            }
            (_, SpendExecutorClaimStatus::Claimed) => false,
        };
        let reconciliation_replay_is_consistent = match &authority {
            ExecutorFinalizationAuthority::Executor => true,
            ExecutorFinalizationAuthority::Reconciliation {
                provider_reference,
                evidence,
                reconciled_by_user_id,
            } => {
                claim.provider_reference.as_ref() == Some(provider_reference)
                    && claim.reconciliation_evidence.as_ref() == Some(evidence)
                    && claim.reconciled_at.is_some()
                    && claim.reconciled_by_user_id.as_ref() == Some(reconciled_by_user_id)
            }
        };
        if replay_is_consistent && !reconciliation_replay_is_consistent {
            let message = if claim.reconciled_at.is_some() {
                "executor claim was reconciled with different evidence"
            } else {
                "executor claim was finalized without reconciliation"
            };
            return Err(StorageError::InvalidData(message.to_string()));
        }
        if replay_is_consistent {
            sqlite_tx.commit()?;
            return Ok(ExecutorFinalizationResult {
                claim,
                token,
                hold,
                balance,
                receipt: persisted_receipt,
                idempotent_replay: true,
            });
        }
        if !matches!(claim.status, SpendExecutorClaimStatus::Claimed) {
            return Err(StorageError::InvalidData(
                "finalized executor claim has inconsistent persisted state".to_string(),
            ));
        }
        if persisted_receipt.is_some() {
            return Err(StorageError::InvalidData(
                "unsettled executor claim already has a settlement receipt".to_string(),
            ));
        }
        match &authority {
            ExecutorFinalizationAuthority::Executor
                if claim.expires_at <= finalization_started_at =>
            {
                return Err(StorageError::InvalidData(
                    "executor claim expired and requires reconciliation".to_string(),
                ));
            }
            ExecutorFinalizationAuthority::Reconciliation { .. }
                if claim.expires_at > finalization_started_at =>
            {
                return Err(StorageError::InvalidData(
                    "active executor claim does not require reconciliation".to_string(),
                ));
            }
            _ => {}
        }
        if token.owner_user_id != claim.owner_user_id {
            return Err(StorageError::InvalidData(
                "executor claim token owner does not match".to_string(),
            ));
        }
        if token.used_at.is_some() || token.used_by_payment_id.is_some() {
            return Err(StorageError::InvalidData(
                "executor claim token has already been used".to_string(),
            ));
        }
        if token.revoked_at.is_some() {
            return Err(StorageError::InvalidData(
                "executor claim token has been revoked".to_string(),
            ));
        }
        if !matches!(hold.status, BudgetHoldStatus::Claimed)
            || hold.executor_claim_id.as_ref() != Some(&claim.id)
            || hold.spend_decision_id != token.spend_decision_id
        {
            return Err(StorageError::InvalidData(
                "executor claim budget hold is not exclusively claimed".to_string(),
            ));
        }
        if balance.frozen_amount_cents < hold.amount_cents {
            return Err(StorageError::InvalidData(
                "executor claim budget balance is inconsistent".to_string(),
            ));
        }
        if let ExecutorFinalizationAction::Settle { receipt, .. } = &action {
            validate_executor_settlement_receipt(receipt, &hold)?;
        }

        let finalized_at = finalization_started_at.to_rfc3339();
        let (terminal_status, settlement_id, transition_name) = match &action {
            ExecutorFinalizationAction::Settle { settlement_id, .. } => {
                ("settled", Some(settlement_id.to_string()), "settlement")
            }
            ExecutorFinalizationAction::Release => ("released", None, "release"),
        };
        let token_rows = match &action {
            ExecutorFinalizationAction::Settle { settlement_id, .. } => sqlite_tx.execute(
                "UPDATE spend_auth_tokens
                 SET used_at = ?2, used_by_payment_id = ?3
                 WHERE id = ?1 AND used_at IS NULL AND revoked_at IS NULL",
                params![
                    claim.spend_auth_token_id.to_string(),
                    finalized_at,
                    settlement_id.to_string(),
                ],
            )?,
            ExecutorFinalizationAction::Release => sqlite_tx.execute(
                "UPDATE spend_auth_tokens
                 SET revoked_at = ?2
                 WHERE id = ?1 AND used_at IS NULL AND revoked_at IS NULL",
                params![claim.spend_auth_token_id.to_string(), finalized_at],
            )?,
        };
        require_one_updated_row(
            token_rows,
            &format!("executor claim token changed during {transition_name}"),
        )?;
        let (provider_reference, reconciliation_evidence, reconciled_at, reconciled_by_user_id) =
            match &authority {
                ExecutorFinalizationAuthority::Executor => (None, None, None, None),
                ExecutorFinalizationAuthority::Reconciliation {
                    provider_reference,
                    evidence,
                    reconciled_by_user_id,
                } => (
                    Some(provider_reference.clone()),
                    Some(evidence.clone()),
                    Some(finalized_at.clone()),
                    Some(reconciled_by_user_id.to_string()),
                ),
            };
        require_one_updated_row(
            sqlite_tx.execute(
                "UPDATE spend_executor_claims
                 SET status = ?2, finalized_at = ?3, settlement_id = ?4,
                     provider_reference = ?5, reconciliation_evidence = ?6,
                     reconciled_at = ?7, reconciled_by_user_id = ?8
                 WHERE id = ?1 AND status = 'claimed'",
                params![
                    claim.id.to_string(),
                    terminal_status,
                    finalized_at,
                    settlement_id,
                    provider_reference,
                    reconciliation_evidence,
                    reconciled_at,
                    reconciled_by_user_id,
                ],
            )?,
            &format!("executor claim changed during {transition_name}"),
        )?;
        require_one_updated_row(
            sqlite_tx.execute(
                "UPDATE budget_holds
                 SET status = ?2, updated_at = ?3
                 WHERE id = ?1 AND status = 'claimed' AND executor_claim_id = ?4",
                params![
                    hold.id.to_string(),
                    terminal_status,
                    finalized_at,
                    claim.id.to_string(),
                ],
            )?,
            &format!("executor claim budget hold changed during {transition_name}"),
        )?;
        let balance_rows = match &action {
            ExecutorFinalizationAction::Settle { receipt, .. } => sqlite_tx.execute(
                "UPDATE budget_balances
                 SET frozen_amount_cents = frozen_amount_cents - ?2,
                     consumed_amount_cents = consumed_amount_cents + ?3,
                     remaining_amount_cents = remaining_amount_cents + (?2 - ?3),
                     updated_at = ?4
                 WHERE budget_id = ?1 AND frozen_amount_cents >= ?2",
                params![
                    hold.budget_id.to_string(),
                    hold.amount_cents,
                    receipt.actual_vendor_cost_cents,
                    finalized_at
                ],
            )?,
            ExecutorFinalizationAction::Release => sqlite_tx.execute(
                "UPDATE budget_balances
                 SET frozen_amount_cents = frozen_amount_cents - ?2,
                     remaining_amount_cents = remaining_amount_cents + ?2,
                     updated_at = ?3
                 WHERE budget_id = ?1 AND frozen_amount_cents >= ?2",
                params![hold.budget_id.to_string(), hold.amount_cents, finalized_at],
            )?,
        };
        require_one_updated_row(
            balance_rows,
            &format!("executor claim budget balance changed during {transition_name}"),
        )?;
        refresh_persisted_budget_status(
            &sqlite_tx,
            &hold.budget_id.to_string(),
            finalization_started_at,
        )?;

        claim.finalized_at = Some(finalization_started_at);
        if let ExecutorFinalizationAuthority::Reconciliation {
            provider_reference,
            evidence,
            reconciled_by_user_id,
        } = authority
        {
            claim.provider_reference = Some(provider_reference);
            claim.reconciliation_evidence = Some(evidence);
            claim.reconciled_at = Some(finalization_started_at);
            claim.reconciled_by_user_id = Some(reconciled_by_user_id);
        }
        hold.updated_at = finalization_started_at;
        balance.frozen_amount_cents -= hold.amount_cents;
        match action {
            ExecutorFinalizationAction::Settle {
                settlement_id,
                receipt,
            } => {
                let released_amount_cents = hold.amount_cents - receipt.actual_vendor_cost_cents;
                let receipt_record = PersistedSpendExecutorSettlementReceipt {
                    claim_id: claim.id.clone(),
                    settlement_id: settlement_id.clone(),
                    authorized_max_cents: hold.amount_cents,
                    released_amount_cents,
                    currency: hold.currency,
                    receipt,
                    created_at: finalization_started_at,
                };
                save_executor_settlement_receipt(&sqlite_tx, &receipt_record)?;
                token.used_at = Some(finalization_started_at);
                token.used_by_payment_id = Some(settlement_id.clone());
                claim.status = SpendExecutorClaimStatus::Settled;
                claim.settlement_id = Some(settlement_id);
                hold.status = BudgetHoldStatus::Settled;
                balance.consumed_amount_cents += receipt_record.receipt.actual_vendor_cost_cents;
                balance.remaining_amount_cents += released_amount_cents;
                persisted_receipt = Some(receipt_record);
            }
            ExecutorFinalizationAction::Release => {
                token.revoked_at = Some(finalization_started_at);
                claim.status = SpendExecutorClaimStatus::Released;
                claim.settlement_id = None;
                hold.status = BudgetHoldStatus::Released;
                balance.remaining_amount_cents += hold.amount_cents;
            }
        }

        sqlite_tx.commit()?;
        Ok(ExecutorFinalizationResult {
            claim,
            token,
            hold,
            balance,
            receipt: persisted_receipt,
            idempotent_replay: false,
        })
    }

    fn from_connection(conn: Connection) -> Result<Self, StorageError> {
        let mut repository = Self { conn };
        repository.init()?;
        Ok(repository)
    }

    fn init(&mut self) -> Result<(), StorageError> {
        self.conn.execute_batch(
            "
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS policy_assignments (
                owner_user_id TEXT NOT NULL,
                scope_type TEXT NOT NULL,
                scope_id TEXT NOT NULL,
                agent_id TEXT,
                policy_id TEXT NOT NULL,
                policy_version TEXT NOT NULL,
                policy_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY(owner_user_id, scope_type, scope_id)
            );

            CREATE TABLE IF NOT EXISTS policies (
                policy_id TEXT PRIMARY KEY,
                owner_user_id TEXT NOT NULL,
                declarative_key TEXT NOT NULL,
                display_name TEXT NOT NULL,
                current_revision INTEGER NOT NULL,
                current_hash TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE(owner_user_id, declarative_key)
            );

            CREATE TABLE IF NOT EXISTS policy_revisions (
                policy_id TEXT NOT NULL,
                revision INTEGER NOT NULL,
                payload_hash TEXT NOT NULL,
                policy_json TEXT NOT NULL,
                actor TEXT NOT NULL,
                source TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY(policy_id, revision),
                FOREIGN KEY(policy_id) REFERENCES policies(policy_id)
            );

            CREATE TABLE IF NOT EXISTS policy_audit_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                policy_id TEXT NOT NULL,
                actor TEXT NOT NULL,
                source TEXT NOT NULL,
                occurred_at TEXT NOT NULL,
                old_hash TEXT,
                new_hash TEXT NOT NULL,
                affected_assignments_json TEXT NOT NULL,
                action TEXT NOT NULL,
                FOREIGN KEY(policy_id) REFERENCES policies(policy_id)
            );

            CREATE TABLE IF NOT EXISTS spend_decisions (
                id TEXT PRIMARY KEY,
                owner_user_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                operation_key TEXT NOT NULL,
                revision INTEGER NOT NULL DEFAULT 1,
                actor TEXT NOT NULL DEFAULT 'system:legacy',
                request_json TEXT NOT NULL,
                evaluation_json TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS spend_operation_attempts (
                agent_id TEXT NOT NULL,
                operation_key TEXT NOT NULL,
                revision INTEGER NOT NULL,
                owner_user_id TEXT NOT NULL,
                request_json TEXT NOT NULL,
                actor TEXT NOT NULL,
                submitted_at TEXT NOT NULL,
                PRIMARY KEY(agent_id, operation_key, revision)
            );

            CREATE TABLE IF NOT EXISTS spend_authorization_outcomes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent_id TEXT NOT NULL,
                operation_key TEXT NOT NULL,
                revision INTEGER NOT NULL,
                spend_decision_id TEXT,
                decision TEXT NOT NULL,
                reasons_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY(agent_id, operation_key, revision)
                    REFERENCES spend_operation_attempts(agent_id, operation_key, revision),
                FOREIGN KEY(spend_decision_id) REFERENCES spend_decisions(id)
            );

            CREATE TABLE IF NOT EXISTS spend_auth_tokens (
                id TEXT PRIMARY KEY,
                owner_user_id TEXT NOT NULL,
                spend_decision_id TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                claim_ttl_seconds INTEGER NOT NULL DEFAULT 900,
                used_at TEXT,
                used_by_payment_id TEXT,
                revoked_at TEXT,
                FOREIGN KEY(spend_decision_id) REFERENCES spend_decisions(id)
            );

            CREATE TABLE IF NOT EXISTS spend_executor_claims (
                id TEXT PRIMARY KEY,
                spend_auth_token_id TEXT NOT NULL UNIQUE,
                owner_user_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                operation_key TEXT NOT NULL,
                workload_profile TEXT NOT NULL,
                status TEXT NOT NULL,
                claimed_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                finalized_at TEXT,
                settlement_id TEXT,
                provider_reference TEXT,
                reconciliation_evidence TEXT,
                reconciled_at TEXT,
                reconciled_by_user_id TEXT,
                FOREIGN KEY(spend_auth_token_id) REFERENCES spend_auth_tokens(id)
            );

            CREATE TABLE IF NOT EXISTS spend_executor_settlement_receipts (
                claim_id TEXT PRIMARY KEY,
                settlement_id TEXT NOT NULL UNIQUE,
                authorized_max_cents INTEGER NOT NULL,
                actual_vendor_cost_cents INTEGER NOT NULL,
                released_amount_cents INTEGER NOT NULL,
                currency TEXT NOT NULL,
                provider_request_id TEXT NOT NULL,
                price_model_snapshot_json TEXT NOT NULL,
                artifact_reference TEXT NOT NULL,
                created_at TEXT NOT NULL,
                CHECK(authorized_max_cents > 0),
                CHECK(actual_vendor_cost_cents >= 0),
                CHECK(actual_vendor_cost_cents <= authorized_max_cents),
                CHECK(released_amount_cents = authorized_max_cents - actual_vendor_cost_cents),
                FOREIGN KEY(claim_id) REFERENCES spend_executor_claims(id)
            );

            CREATE TABLE IF NOT EXISTS budgets (
                id TEXT PRIMARY KEY,
                scope_type TEXT NOT NULL,
                scope_id TEXT NOT NULL,
                amount_limit_cents INTEGER NOT NULL,
                currency TEXT NOT NULL,
                starting_at TEXT NOT NULL,
                ending_before TEXT,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS spending_targets (
                id TEXT PRIMARY KEY,
                owner_user_id TEXT NOT NULL,
                target_amount_cents INTEGER NOT NULL,
                currency TEXT NOT NULL,
                starting_at TEXT NOT NULL,
                ending_before TEXT,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS budget_balances (
                budget_id TEXT PRIMARY KEY,
                consumed_amount_cents INTEGER NOT NULL,
                frozen_amount_cents INTEGER NOT NULL,
                remaining_amount_cents INTEGER NOT NULL,
                updated_at TEXT NOT NULL,
                FOREIGN KEY(budget_id) REFERENCES budgets(id)
            );

            CREATE TABLE IF NOT EXISTS budget_holds (
                id TEXT PRIMARY KEY,
                budget_id TEXT NOT NULL,
                spend_decision_id TEXT NOT NULL,
                amount_cents INTEGER NOT NULL,
                currency TEXT NOT NULL,
                status TEXT NOT NULL,
                executor_claim_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                FOREIGN KEY(budget_id) REFERENCES budgets(id),
                FOREIGN KEY(spend_decision_id) REFERENCES spend_decisions(id)
            );

            CREATE TRIGGER IF NOT EXISTS spend_decisions_no_update
            BEFORE UPDATE ON spend_decisions
            BEGIN
                SELECT RAISE(ABORT, 'spend decisions are immutable');
            END;

            CREATE TRIGGER IF NOT EXISTS spend_decisions_no_delete
            BEFORE DELETE ON spend_decisions
            BEGIN
                SELECT RAISE(ABORT, 'spend decisions are immutable');
            END;


            CREATE TRIGGER IF NOT EXISTS policy_revisions_no_update
            BEFORE UPDATE ON policy_revisions
            BEGIN
                SELECT RAISE(ABORT, 'policy revisions are immutable');
            END;

            CREATE TRIGGER IF NOT EXISTS policy_revisions_no_delete
            BEFORE DELETE ON policy_revisions
            BEGIN
                SELECT RAISE(ABORT, 'policy revisions are immutable');
            END;

            CREATE TRIGGER IF NOT EXISTS policy_audit_events_no_update
            BEFORE UPDATE ON policy_audit_events
            BEGIN
                SELECT RAISE(ABORT, 'policy audit events are immutable');
            END;

            CREATE TRIGGER IF NOT EXISTS policy_audit_events_no_delete
            BEFORE DELETE ON policy_audit_events
            BEGIN
                SELECT RAISE(ABORT, 'policy audit events are immutable');
            END;
            ",
        )?;
        self.migrate_policy_assignment_scope()?;
        self.migrate_declarative_policies()?;
        self.migrate_user_caps_to_spending_targets()?;
        self.migrate_executor_claim_budget_holds()?;
        self.migrate_spend_auth_token_claim_ttl()?;
        self.migrate_spend_operation_keys()?;
        self.migrate_spend_operation_attempts()?;
        self.migrate_executor_claim_reconciliation()?;
        self.enforce_one_budget_hold_per_spend_decision()?;
        Ok(())
    }

    fn migrate_policy_assignment_scope(&self) -> Result<(), StorageError> {
        if table_has_column(&self.conn, "policy_assignments", "scope_type")? {
            return Ok(());
        }

        self.conn.execute_batch(
            "
            ALTER TABLE policy_assignments RENAME TO policy_assignments_legacy;

            CREATE TABLE policy_assignments (
                owner_user_id TEXT NOT NULL,
                scope_type TEXT NOT NULL,
                scope_id TEXT NOT NULL,
                agent_id TEXT,
                policy_id TEXT NOT NULL,
                policy_version TEXT NOT NULL,
                policy_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY(owner_user_id, scope_type, scope_id)
            );

            INSERT INTO policy_assignments
                (owner_user_id, scope_type, scope_id, agent_id, policy_id, policy_version, policy_json, created_at, updated_at)
            SELECT
                owner_user_id, 'agent_override', agent_id, agent_id, policy_id, policy_version, policy_json, created_at, updated_at
            FROM policy_assignments_legacy;

            DROP TABLE policy_assignments_legacy;
            ",
        )?;
        Ok(())
    }

    fn migrate_declarative_policies(&mut self) -> Result<(), StorageError> {
        if !table_has_column(&self.conn, "policy_assignments", "policy_json")? {
            return Ok(());
        }

        #[derive(Debug)]
        struct LegacyAssignment {
            owner_user_id: String,
            scope_type: String,
            scope_id: String,
            agent_id: Option<String>,
            policy_json: String,
            created_at: String,
            updated_at: String,
        }

        let legacy = {
            let mut statement = self.conn.prepare(
                "SELECT owner_user_id, scope_type, scope_id, agent_id, policy_json, created_at, updated_at
                 FROM policy_assignments ORDER BY created_at, owner_user_id, scope_type, scope_id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(LegacyAssignment {
                    owner_user_id: row.get(0)?,
                    scope_type: row.get(1)?,
                    scope_id: row.get(2)?,
                    agent_id: row.get(3)?,
                    policy_json: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            })?;
            collect_rows(rows)?
        };

        let transaction = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(
            "ALTER TABLE policy_assignments RENAME TO policy_assignments_embedded_legacy;
             CREATE TABLE policy_assignments (
                owner_user_id TEXT NOT NULL,
                scope_type TEXT NOT NULL,
                scope_id TEXT NOT NULL,
                agent_id TEXT,
                policy_id TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY(owner_user_id, scope_type, scope_id),
                FOREIGN KEY(policy_id) REFERENCES policies(policy_id)
             );",
        )?;

        for assignment in legacy {
            let policy: Policy = serde_json::from_str(&assignment.policy_json)?;
            let payload_hash = policy_payload_hash(&policy)?;
            let mut declarative_key = legacy_declarative_key(&policy.id, &payload_hash);
            let existing =
                policy_identity_for_key(&transaction, &assignment.owner_user_id, &declarative_key)?;

            let policy_id = match existing {
                Some((policy_id, hash)) if hash == payload_hash => policy_id,
                Some(_) => {
                    let hash_hex = payload_hash
                        .strip_prefix("sha256:")
                        .ok_or_else(|| StorageError::InvalidData("invalid policy hash".into()))?;
                    declarative_key = format!(
                        "{}--migrated-{hash_hex}",
                        declarative_key.chars().take(53).collect::<String>(),
                    );
                    match policy_identity_for_key(
                        &transaction,
                        &assignment.owner_user_id,
                        &declarative_key,
                    )? {
                        Some((policy_id, hash)) if hash == payload_hash => policy_id,
                        Some(_) => {
                            return Err(StorageError::InvalidData(format!(
                                "legacy policy migration key collision for `{declarative_key}`"
                            )));
                        }
                        None => new_public_policy_id(&transaction)?,
                    }
                }
                None => new_public_policy_id(&transaction)?,
            };

            let exists: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM policies WHERE policy_id = ?1)",
                params![policy_id],
                |row| row.get(0),
            )?;
            if !exists {
                transaction.execute(
                    "INSERT INTO policies
                     (policy_id, owner_user_id, declarative_key, display_name, current_revision,
                      current_hash, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7)",
                    params![
                        policy_id,
                        assignment.owner_user_id,
                        declarative_key,
                        policy.id,
                        payload_hash,
                        assignment.created_at,
                        assignment.updated_at,
                    ],
                )?;
                transaction.execute(
                    "INSERT INTO policy_revisions
                     (policy_id, revision, payload_hash, policy_json, actor, source, created_at)
                     VALUES (?1, 1, ?2, ?3, 'system:migration', 'legacy_assignment_migration', ?4)",
                    params![
                        policy_id,
                        payload_hash,
                        assignment.policy_json,
                        assignment.created_at
                    ],
                )?;
            }

            let affected = serde_json::to_string(&PolicyAuditAssignments(vec![format!(
                "{}:{}",
                assignment.scope_type, assignment.scope_id
            )]))?;
            transaction.execute(
                "INSERT INTO policy_assignments
                 (owner_user_id, scope_type, scope_id, agent_id, policy_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    assignment.owner_user_id,
                    assignment.scope_type,
                    assignment.scope_id,
                    assignment.agent_id,
                    policy_id,
                    assignment.created_at,
                    assignment.updated_at,
                ],
            )?;
            transaction.execute(
                "INSERT INTO policy_audit_events
                 (policy_id, actor, source, occurred_at, old_hash, new_hash,
                  affected_assignments_json, action)
                 VALUES (?1, 'system:migration', 'legacy_assignment_migration', ?2,
                         ?3, ?4, ?5, 'migrated')",
                params![
                    policy_id,
                    assignment.updated_at,
                    exists.then_some(payload_hash.as_str()),
                    payload_hash,
                    affected
                ],
            )?;
        }

        transaction.execute_batch("DROP TABLE policy_assignments_embedded_legacy;")?;
        transaction.commit()?;
        Ok(())
    }

    fn migrate_user_caps_to_spending_targets(&self) -> Result<(), StorageError> {
        self.conn.execute_batch(
            "
            INSERT OR IGNORE INTO spending_targets
                (id, owner_user_id, target_amount_cents, currency, starting_at,
                 ending_before, status, created_at, updated_at)
            SELECT
                id,
                scope_id,
                amount_limit_cents,
                currency,
                starting_at,
                ending_before,
                CASE WHEN status = 'revoked' THEN 'revoked' ELSE 'active' END,
                created_at,
                updated_at
            FROM budgets
            WHERE scope_type = 'user';

            DELETE FROM budget_holds
            WHERE budget_id IN (SELECT id FROM budgets WHERE scope_type = 'user');

            DELETE FROM budget_balances
            WHERE budget_id IN (SELECT id FROM budgets WHERE scope_type = 'user');

            DELETE FROM budgets
            WHERE scope_type = 'user';
            ",
        )?;
        Ok(())
    }

    fn enforce_one_budget_hold_per_spend_decision(&self) -> Result<(), StorageError> {
        self.conn.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS budget_holds_one_per_spend_decision
             ON budget_holds(spend_decision_id)",
            [],
        )?;
        Ok(())
    }

    fn migrate_executor_claim_budget_holds(&self) -> Result<(), StorageError> {
        if !table_has_column(&self.conn, "budget_holds", "executor_claim_id")? {
            self.conn.execute(
                "ALTER TABLE budget_holds ADD COLUMN executor_claim_id TEXT",
                [],
            )?;
        }
        self.conn.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS budget_holds_executor_claim_unique
             ON budget_holds(executor_claim_id)
             WHERE executor_claim_id IS NOT NULL;",
        )?;
        Ok(())
    }

    fn migrate_spend_auth_token_claim_ttl(&self) -> Result<(), StorageError> {
        if !table_has_column(&self.conn, "spend_auth_tokens", "claim_ttl_seconds")? {
            self.conn.execute(
                "ALTER TABLE spend_auth_tokens
                 ADD COLUMN claim_ttl_seconds INTEGER NOT NULL DEFAULT 900",
                [],
            )?;
        }
        Ok(())
    }

    fn migrate_spend_operation_keys(&self) -> Result<(), StorageError> {
        self.conn.execute_batch(
            "DROP TRIGGER IF EXISTS spend_decisions_no_update;
             DROP TRIGGER IF EXISTS spend_decisions_no_delete;
             DROP TRIGGER IF EXISTS spend_executor_claims_workflow_matches;
             DROP TRIGGER IF EXISTS spend_executor_claims_identity_immutable;
             DROP TRIGGER IF EXISTS spend_decisions_job_id_required;
             DROP TRIGGER IF EXISTS spend_executor_claims_job_id_required;
             DROP TRIGGER IF EXISTS spend_executor_claims_operation_key_required;
             DROP INDEX IF EXISTS spend_decisions_owner_job_unique;
             DROP INDEX IF EXISTS spend_executor_claims_owner_job_unique;
             DROP INDEX IF EXISTS spend_decisions_agent_job_unique;
             DROP INDEX IF EXISTS spend_executor_claims_agent_job_unique;
             DROP INDEX IF EXISTS spend_decisions_agent_operation_unique;
             DROP INDEX IF EXISTS spend_executor_claims_agent_operation_unique;",
        )?;
        let decisions_had_operation_key =
            table_has_column(&self.conn, "spend_decisions", "operation_key")?;
        let decisions_had_job_id = table_has_column(&self.conn, "spend_decisions", "job_id")?;
        let claims_had_operation_key =
            table_has_column(&self.conn, "spend_executor_claims", "operation_key")?;
        let claims_had_job_id = table_has_column(&self.conn, "spend_executor_claims", "job_id")?;
        let claims_had_execution_id =
            table_has_column(&self.conn, "spend_executor_claims", "executor_execution_id")?;

        if !table_has_column(&self.conn, "spend_decisions", "agent_id")? {
            self.conn
                .execute("ALTER TABLE spend_decisions ADD COLUMN agent_id TEXT", [])?;
        }
        if !decisions_had_operation_key {
            self.conn.execute(
                "ALTER TABLE spend_decisions ADD COLUMN operation_key TEXT",
                [],
            )?;
        }
        if !table_has_column(&self.conn, "spend_executor_claims", "agent_id")? {
            self.conn.execute(
                "ALTER TABLE spend_executor_claims ADD COLUMN agent_id TEXT",
                [],
            )?;
        }
        if !claims_had_operation_key {
            self.conn.execute(
                "ALTER TABLE spend_executor_claims ADD COLUMN operation_key TEXT",
                [],
            )?;
        }

        self.conn.execute_batch(
            "UPDATE spend_decisions
             SET agent_id = json_extract(request_json, '$.agent_id')
             WHERE agent_id IS NULL OR trim(agent_id) = '';",
        )?;

        if !decisions_had_operation_key {
            if decisions_had_job_id {
                self.conn.execute_batch(
                    "UPDATE spend_decisions
                     SET operation_key = job_id
                     WHERE job_id IS NOT NULL AND trim(job_id) != '';",
                )?;
            } else if claims_had_operation_key {
                self.copy_legacy_claim_identifier_to_decisions("operation_key")?;
            } else if claims_had_job_id {
                self.copy_legacy_claim_identifier_to_decisions("job_id")?;
            } else if claims_had_execution_id {
                self.copy_legacy_claim_identifier_to_decisions("executor_execution_id")?;
            }
        }

        self.conn.execute_batch(
            "UPDATE spend_decisions
             SET operation_key = 'legacy:' || id
             WHERE operation_key IS NULL OR trim(operation_key) = '';",
        )?;

        if !decisions_had_operation_key {
            self.conn.execute_batch(
                "UPDATE spend_decisions
                 SET operation_key = operation_key || ':legacy:' || id
                 WHERE id IN (
                     SELECT id
                     FROM (
                         SELECT id,
                                ROW_NUMBER() OVER (
                                    PARTITION BY agent_id, operation_key
                                    ORDER BY created_at, id
                                ) AS duplicate_rank
                         FROM spend_decisions
                     )
                     WHERE duplicate_rank > 1
                 );",
            )?;
        }

        self.conn.execute_batch(
            "UPDATE spend_executor_claims AS claims
             SET agent_id = (
                 SELECT decisions.agent_id
                 FROM spend_auth_tokens AS tokens
                 JOIN spend_decisions AS decisions
                   ON decisions.id = tokens.spend_decision_id
                 WHERE tokens.id = claims.spend_auth_token_id
             )
             WHERE agent_id IS NULL OR trim(agent_id) = '';

             UPDATE spend_executor_claims AS claims
             SET operation_key = (
                 SELECT decisions.operation_key
                 FROM spend_auth_tokens AS tokens
                 JOIN spend_decisions AS decisions
                   ON decisions.id = tokens.spend_decision_id
                 WHERE tokens.id = claims.spend_auth_token_id
             )
             WHERE EXISTS (
                 SELECT 1
                 FROM spend_auth_tokens AS tokens
                 WHERE tokens.id = claims.spend_auth_token_id
             );

             UPDATE spend_executor_claims
             SET operation_key = 'legacy:' || id
             WHERE operation_key IS NULL OR trim(operation_key) = '';",
        )?;

        if decisions_had_job_id {
            self.conn
                .execute("ALTER TABLE spend_decisions DROP COLUMN job_id", [])?;
        }
        if claims_had_job_id {
            self.conn
                .execute("ALTER TABLE spend_executor_claims DROP COLUMN job_id", [])?;
        }
        if claims_had_execution_id {
            self.conn.execute(
                "ALTER TABLE spend_executor_claims DROP COLUMN executor_execution_id",
                [],
            )?;
        }

        self.conn.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS spend_decisions_agent_operation_unique
             ON spend_decisions(agent_id, operation_key);

             CREATE UNIQUE INDEX IF NOT EXISTS spend_executor_claims_agent_operation_unique
             ON spend_executor_claims(agent_id, operation_key);

             CREATE TRIGGER IF NOT EXISTS spend_decisions_agent_id_required
             BEFORE INSERT ON spend_decisions
             WHEN NEW.agent_id IS NULL OR trim(NEW.agent_id) = ''
             BEGIN
                 SELECT RAISE(ABORT, 'spend decision agent id is required');
             END;

             CREATE TRIGGER IF NOT EXISTS spend_decisions_operation_key_required
             BEFORE INSERT ON spend_decisions
             WHEN NEW.operation_key IS NULL OR trim(NEW.operation_key) = ''
             BEGIN
                 SELECT RAISE(ABORT, 'spend decision operation key is required');
             END;

             CREATE TRIGGER IF NOT EXISTS spend_executor_claims_operation_key_required
             BEFORE INSERT ON spend_executor_claims
             WHEN NEW.operation_key IS NULL OR trim(NEW.operation_key) = ''
             BEGIN
                 SELECT RAISE(ABORT, 'executor claim operation key is required');
             END;

             CREATE TRIGGER IF NOT EXISTS spend_executor_claims_agent_id_required
             BEFORE INSERT ON spend_executor_claims
             WHEN NEW.agent_id IS NULL OR trim(NEW.agent_id) = ''
             BEGIN
                 SELECT RAISE(ABORT, 'executor claim agent id is required');
             END;

             CREATE TRIGGER IF NOT EXISTS spend_executor_claims_workflow_matches
             BEFORE INSERT ON spend_executor_claims
             WHEN NOT EXISTS (
                 SELECT 1
                 FROM spend_auth_tokens AS tokens
                 JOIN spend_decisions AS decisions
                   ON decisions.id = tokens.spend_decision_id
                 WHERE tokens.id = NEW.spend_auth_token_id
                   AND decisions.owner_user_id = NEW.owner_user_id
                   AND decisions.agent_id = NEW.agent_id
                   AND decisions.operation_key = NEW.operation_key
             )
             BEGIN
                 SELECT RAISE(ABORT, 'executor claim does not match authorized operation');
             END;

             CREATE TRIGGER IF NOT EXISTS spend_executor_claims_identity_immutable
             BEFORE UPDATE OF spend_auth_token_id, owner_user_id, agent_id, operation_key
             ON spend_executor_claims
             BEGIN
                 SELECT RAISE(ABORT, 'executor claim identity is immutable');
             END;

             CREATE TRIGGER IF NOT EXISTS spend_decisions_no_update
             BEFORE UPDATE ON spend_decisions
             BEGIN
                 SELECT RAISE(ABORT, 'spend decisions are immutable');
             END;

             CREATE TRIGGER IF NOT EXISTS spend_decisions_no_delete
             BEFORE DELETE ON spend_decisions
             BEGIN
                 SELECT RAISE(ABORT, 'spend decisions are immutable');
             END;",
        )?;
        Ok(())
    }

    fn copy_legacy_claim_identifier_to_decisions(
        &self,
        claim_column: &str,
    ) -> Result<(), StorageError> {
        self.conn.execute_batch(&format!(
            "UPDATE spend_decisions AS decisions
             SET operation_key = (
                 SELECT claims.{claim_column}
                 FROM spend_auth_tokens AS tokens
                 JOIN spend_executor_claims AS claims
                   ON claims.spend_auth_token_id = tokens.id
                 WHERE tokens.spend_decision_id = decisions.id
                 ORDER BY claims.claimed_at, claims.id
                 LIMIT 1
             )
             WHERE EXISTS (
                 SELECT 1
                 FROM spend_auth_tokens AS tokens
                 JOIN spend_executor_claims AS claims
                   ON claims.spend_auth_token_id = tokens.id
                 WHERE tokens.spend_decision_id = decisions.id
                   AND claims.{claim_column} IS NOT NULL
                   AND trim(claims.{claim_column}) != ''
             );"
        ))?;
        Ok(())
    }

    fn migrate_executor_claim_reconciliation(&self) -> Result<(), StorageError> {
        for (column, column_type) in [
            ("provider_reference", "TEXT"),
            ("reconciliation_evidence", "TEXT"),
            ("reconciled_at", "TEXT"),
            ("reconciled_by_user_id", "TEXT"),
        ] {
            if !table_has_column(&self.conn, "spend_executor_claims", column)? {
                self.conn.execute(
                    &format!("ALTER TABLE spend_executor_claims ADD COLUMN {column} {column_type}"),
                    [],
                )?;
            }
        }
        Ok(())
    }

    fn migrate_spend_operation_attempts(&self) -> Result<(), StorageError> {
        if !table_has_column(&self.conn, "spend_decisions", "revision")? {
            self.conn.execute(
                "ALTER TABLE spend_decisions ADD COLUMN revision INTEGER NOT NULL DEFAULT 1",
                [],
            )?;
        }
        if !table_has_column(&self.conn, "spend_decisions", "actor")? {
            self.conn.execute(
                "ALTER TABLE spend_decisions ADD COLUMN actor TEXT NOT NULL DEFAULT 'system:migration'",
                [],
            )?;
        }
        self.conn.execute_batch(
            "DROP INDEX IF EXISTS spend_decisions_agent_operation_unique;
             CREATE UNIQUE INDEX IF NOT EXISTS spend_decisions_agent_operation_revision_unique
             ON spend_decisions(agent_id, operation_key, revision);

             CREATE TABLE IF NOT EXISTS spend_operation_attempts (
                agent_id TEXT NOT NULL,
                operation_key TEXT NOT NULL,
                revision INTEGER NOT NULL,
                owner_user_id TEXT NOT NULL,
                request_json TEXT NOT NULL,
                actor TEXT NOT NULL,
                submitted_at TEXT NOT NULL,
                PRIMARY KEY(agent_id, operation_key, revision)
             );
             CREATE TABLE IF NOT EXISTS spend_authorization_outcomes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent_id TEXT NOT NULL,
                operation_key TEXT NOT NULL,
                revision INTEGER NOT NULL,
                spend_decision_id TEXT,
                decision TEXT NOT NULL,
                reasons_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY(agent_id, operation_key, revision)
                    REFERENCES spend_operation_attempts(agent_id, operation_key, revision),
                FOREIGN KEY(spend_decision_id) REFERENCES spend_decisions(id)
             );

             INSERT OR IGNORE INTO spend_operation_attempts
                (agent_id, operation_key, revision, owner_user_id, request_json, actor, submitted_at)
             SELECT agent_id, operation_key, revision, owner_user_id, request_json,
                    actor, created_at
             FROM spend_decisions;

             INSERT INTO spend_authorization_outcomes
                (agent_id, operation_key, revision, spend_decision_id, decision,
                 reasons_json, created_at)
             SELECT decisions.agent_id, decisions.operation_key, decisions.revision,
                    decisions.id,
                    CASE
                        WHEN EXISTS (
                            SELECT 1 FROM spend_auth_tokens AS tokens
                            WHERE tokens.spend_decision_id = decisions.id
                        ) THEN 'allowed'
                        WHEN json_extract(decisions.evaluation_json, '$.decision') = 'deny'
                            THEN 'denied'
                        ELSE 'pending_approval'
                    END,
                    COALESCE(json_extract(decisions.evaluation_json, '$.reasons'), '[]'),
                    decisions.created_at
             FROM spend_decisions AS decisions
             WHERE NOT EXISTS (
                SELECT 1 FROM spend_authorization_outcomes AS outcomes
                WHERE outcomes.agent_id = decisions.agent_id
                  AND outcomes.operation_key = decisions.operation_key
                  AND outcomes.revision = decisions.revision
             );

             CREATE TRIGGER IF NOT EXISTS spend_operation_attempts_no_update
             BEFORE UPDATE ON spend_operation_attempts
             BEGIN
                SELECT RAISE(ABORT, 'spend operation attempts are immutable');
             END;
             CREATE TRIGGER IF NOT EXISTS spend_operation_attempts_no_delete
             BEFORE DELETE ON spend_operation_attempts
             BEGIN
                SELECT RAISE(ABORT, 'spend operation attempts are immutable');
             END;
             CREATE TRIGGER IF NOT EXISTS spend_authorization_outcomes_no_update
             BEFORE UPDATE ON spend_authorization_outcomes
             BEGIN
                SELECT RAISE(ABORT, 'spend authorization outcomes are immutable');
             END;
             CREATE TRIGGER IF NOT EXISTS spend_authorization_outcomes_no_delete
             BEFORE DELETE ON spend_authorization_outcomes
             BEGIN
                SELECT RAISE(ABORT, 'spend authorization outcomes are immutable');
             END;
             CREATE TRIGGER IF NOT EXISTS spend_authorization_outcomes_valid_transition
             BEFORE INSERT ON spend_authorization_outcomes
             WHEN EXISTS (
                SELECT 1 FROM spend_authorization_outcomes AS prior
                WHERE prior.agent_id = NEW.agent_id
                  AND prior.operation_key = NEW.operation_key
                  AND prior.revision = NEW.revision
                  AND (
                    NEW.decision = 'pending_approval'
                    OR prior.decision != 'pending_approval'
                  )
             )
             BEGIN
                SELECT RAISE(ABORT, 'spend authorization outcome is already final');
             END;",
        )?;
        Ok(())
    }
}

impl ExecutorClaimRepository for SqliteGovernanceRepository {
    fn save_executor_claim_with_budget_hold(
        &mut self,
        claim: &SpendExecutorClaimRecord,
        hold: &BudgetHold,
        balance: &BudgetBalance,
    ) -> Result<(), StorageError> {
        SqliteGovernanceRepository::save_executor_claim_with_budget_hold(self, claim, hold, balance)
    }

    fn settle_executor_claim_transactionally(
        &mut self,
        owner_user_id: &UserId,
        agent_id: &AgentId,
        operation_key: &str,
        proposed_settlement_id: PaymentId,
        receipt: SpendExecutorSettlementReceipt,
        settlement_started_at: DateTime<Utc>,
    ) -> Result<ExecutorFinalizationResult, StorageError> {
        SqliteGovernanceRepository::settle_executor_claim_transactionally(
            self,
            owner_user_id,
            agent_id,
            operation_key,
            proposed_settlement_id,
            receipt,
            settlement_started_at,
        )
    }

    fn release_executor_claim_transactionally(
        &mut self,
        owner_user_id: &UserId,
        agent_id: &AgentId,
        operation_key: &str,
        finalization_started_at: DateTime<Utc>,
    ) -> Result<ExecutorFinalizationResult, StorageError> {
        SqliteGovernanceRepository::release_executor_claim_transactionally(
            self,
            owner_user_id,
            agent_id,
            operation_key,
            finalization_started_at,
        )
    }

    fn reconcile_executor_claim_as_billed_transactionally(
        &mut self,
        claim_id: &SpendExecutorClaimId,
        owner_user_id: &UserId,
        provider_reference: &str,
        evidence: &str,
        proposed_settlement_id: PaymentId,
        receipt: SpendExecutorSettlementReceipt,
        reconciliation_started_at: DateTime<Utc>,
    ) -> Result<ExecutorFinalizationResult, StorageError> {
        SqliteGovernanceRepository::reconcile_executor_claim_as_billed_transactionally(
            self,
            claim_id,
            owner_user_id,
            provider_reference,
            evidence,
            proposed_settlement_id,
            receipt,
            reconciliation_started_at,
        )
    }

    fn reconcile_executor_claim_as_not_billed_transactionally(
        &mut self,
        claim_id: &SpendExecutorClaimId,
        owner_user_id: &UserId,
        provider_reference: &str,
        evidence: &str,
        reconciliation_started_at: DateTime<Utc>,
    ) -> Result<ExecutorFinalizationResult, StorageError> {
        SqliteGovernanceRepository::reconcile_executor_claim_as_not_billed_transactionally(
            self,
            claim_id,
            owner_user_id,
            provider_reference,
            evidence,
            reconciliation_started_at,
        )
    }
}

impl PolicyRepository for SqliteGovernanceRepository {
    fn apply_policy(
        &mut self,
        owner_user_id: &UserId,
        declarative_key: &str,
        display_name: &str,
        scope: &PolicyAssignmentScope,
        policy: &Policy,
        expected_revision: Option<u64>,
        expected_hash: Option<&str>,
        actor: &str,
        source: &str,
    ) -> Result<PolicyApplyResult, StorageError> {
        let declarative_key = declarative_key.trim();
        let display_name = display_name.trim();
        if declarative_key.is_empty() {
            return Err(StorageError::InvalidData(
                "policy declarative key cannot be empty".to_string(),
            ));
        }
        if !valid_declarative_key(declarative_key) {
            return Err(StorageError::InvalidData(
                "policy declarative key must be 1-128 ASCII letters, digits, `_`, `-`, or `.`"
                    .to_string(),
            ));
        }
        if display_name.is_empty() {
            return Err(StorageError::InvalidData(
                "policy display name cannot be empty".to_string(),
            ));
        }

        let now = Utc::now().to_rfc3339();
        let policy_json = serde_json::to_string(policy)?;
        let payload_hash = policy_payload_hash(policy)?;
        let owner = owner_user_id.to_string();
        let transaction = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<(String, u64, String, String)> = transaction
            .query_row(
                "SELECT policy_id, current_revision, current_hash, display_name
                 FROM policies WHERE owner_user_id = ?1 AND declarative_key = ?2",
                params![owner, declarative_key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;

        let (policy_id, previous_revision, previous_hash, previous_name) = match existing {
            Some(existing) => existing,
            None => {
                if expected_revision.is_some() || expected_hash.is_some() {
                    return Err(StorageError::InvalidData(
                        "policy compare-and-set failed: resource does not exist".to_string(),
                    ));
                }
                (
                    new_public_policy_id(&transaction)?,
                    0,
                    String::new(),
                    String::new(),
                )
            }
        };

        if let Some(expected_revision) = expected_revision {
            if expected_revision != previous_revision {
                return Err(StorageError::InvalidData(format!(
                    "policy compare-and-set failed: expected revision {expected_revision}, current revision is {previous_revision}"
                )));
            }
        }
        if let Some(expected_hash) = expected_hash {
            if expected_hash != previous_hash {
                return Err(StorageError::InvalidData(format!(
                    "policy compare-and-set failed: expected hash {expected_hash}, current hash is {previous_hash}"
                )));
            }
        }

        let content_changed = previous_revision == 0 || previous_hash != payload_hash;
        let name_changed = previous_revision != 0 && previous_name != display_name;
        let revision = if content_changed {
            previous_revision + 1
        } else {
            previous_revision
        };

        if previous_revision == 0 {
            transaction.execute(
                "INSERT INTO policies
                 (policy_id, owner_user_id, declarative_key, display_name, current_revision,
                  current_hash, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![
                    policy_id,
                    owner,
                    declarative_key,
                    display_name,
                    revision,
                    payload_hash,
                    now,
                ],
            )?;
        } else if content_changed || name_changed {
            transaction.execute(
                "UPDATE policies SET display_name = ?1, current_revision = ?2,
                 current_hash = ?3, updated_at = ?4 WHERE policy_id = ?5",
                params![display_name, revision, payload_hash, now, policy_id],
            )?;
        }

        if content_changed {
            transaction.execute(
                "INSERT INTO policy_revisions
                 (policy_id, revision, payload_hash, policy_json, actor, source, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    policy_id,
                    revision,
                    payload_hash,
                    policy_json,
                    actor,
                    source,
                    now
                ],
            )?;
        }

        let prior_assignment: Option<String> = transaction
            .query_row(
                "SELECT policy_id FROM policy_assignments
                 WHERE owner_user_id = ?1 AND scope_type = ?2 AND scope_id = ?3",
                params![owner, scope.scope_type(), scope.scope_id()],
                |row| row.get(0),
            )
            .optional()?;
        let assignment_changed = prior_assignment.as_deref() != Some(policy_id.as_str());
        if assignment_changed {
            transaction.execute(
                "INSERT INTO policy_assignments
                 (owner_user_id, scope_type, scope_id, agent_id, policy_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                 ON CONFLICT(owner_user_id, scope_type, scope_id) DO UPDATE SET
                    agent_id = excluded.agent_id,
                    policy_id = excluded.policy_id,
                    updated_at = excluded.updated_at",
                params![
                    owner,
                    scope.scope_type(),
                    scope.scope_id(),
                    scope.agent_id(),
                    policy_id,
                    now,
                ],
            )?;
        }

        let changed = content_changed || name_changed || assignment_changed;
        if changed {
            let affected = if assignment_changed {
                vec![format!("{}:{}", scope.scope_type(), scope.scope_id())]
            } else {
                Vec::new()
            };
            if let Some(prior_policy_id) = prior_assignment
                .as_deref()
                .filter(|prior_policy_id| *prior_policy_id != policy_id)
            {
                let prior_hash: String = transaction.query_row(
                    "SELECT current_hash FROM policies WHERE policy_id = ?1",
                    params![prior_policy_id],
                    |row| row.get(0),
                )?;
                transaction.execute(
                    "INSERT INTO policy_audit_events
                     (policy_id, actor, source, occurred_at, old_hash, new_hash,
                      affected_assignments_json, action)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, 'unassigned')",
                    params![
                        prior_policy_id,
                        actor,
                        source,
                        now,
                        prior_hash,
                        serde_json::to_string(&PolicyAuditAssignments(affected.clone()))?,
                    ],
                )?;
            }
            transaction.execute(
                "INSERT INTO policy_audit_events
                 (policy_id, actor, source, occurred_at, old_hash, new_hash,
                  affected_assignments_json, action)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    policy_id,
                    actor,
                    source,
                    now,
                    (previous_revision != 0).then_some(previous_hash.as_str()),
                    payload_hash,
                    serde_json::to_string(&PolicyAuditAssignments(affected))?,
                    if content_changed {
                        "applied"
                    } else if name_changed {
                        "renamed"
                    } else {
                        "assigned"
                    },
                ],
            )?;
        }
        transaction.commit()?;

        let resource = self
            .load_policy(owner_user_id, &policy_id)?
            .ok_or_else(|| StorageError::InvalidData("applied policy disappeared".to_string()))?;
        let assignment = self
            .load_policy_assignments()?
            .into_iter()
            .find(|candidate| {
                candidate.owner_user_id == *owner_user_id && candidate.scope == *scope
            })
            .ok_or_else(|| {
                StorageError::InvalidData("applied assignment disappeared".to_string())
            })?;
        Ok(PolicyApplyResult {
            resource,
            assignment,
            changed,
            assignment_changed,
        })
    }

    fn load_policy_assignments(&self) -> Result<Vec<PolicyAssignmentRecord>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT a.owner_user_id, a.scope_type, a.scope_id, a.policy_id,
                    r.policy_json, a.created_at, a.updated_at
             FROM policy_assignments a
             JOIN policies p ON p.policy_id = a.policy_id
             JOIN policy_revisions r
               ON r.policy_id = p.policy_id AND r.revision = p.current_revision
             ORDER BY a.updated_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let owner_user_id: String = row.get(0)?;
            let scope_type: String = row.get(1)?;
            let scope_id: String = row.get(2)?;
            let policy_id: String = row.get(3)?;
            let policy_json: String = row.get(4)?;
            let created_at: String = row.get(5)?;
            let updated_at: String = row.get(6)?;
            let policy: Policy = serde_json::from_str(&policy_json)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            Ok(PolicyAssignmentRecord {
                owner_user_id: parse_id(&owner_user_id)?,
                scope: PolicyAssignmentScope::from_parts(&scope_type, &scope_id).map_err(
                    |error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            2,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    },
                )?,
                policy_id,
                policy,
                created_at: parse_timestamp(&created_at)?,
                updated_at: parse_timestamp(&updated_at)?,
            })
        })?;
        collect_rows(rows)
    }

    fn load_policies(&self, owner_user_id: &UserId) -> Result<Vec<PolicyResource>, StorageError> {
        let mut statement = self.conn.prepare(
            "SELECT p.policy_id, p.owner_user_id, p.declarative_key, p.display_name,
                    p.current_revision, p.current_hash, r.policy_json, p.created_at, p.updated_at
             FROM policies p JOIN policy_revisions r
               ON r.policy_id = p.policy_id AND r.revision = p.current_revision
             WHERE p.owner_user_id = ?1 ORDER BY p.declarative_key",
        )?;
        let rows =
            statement.query_map(params![owner_user_id.to_string()], policy_resource_from_row)?;
        collect_rows(rows)
    }

    fn load_policy(
        &self,
        owner_user_id: &UserId,
        selector: &str,
    ) -> Result<Option<PolicyResource>, StorageError> {
        self.conn
            .query_row(
                "SELECT p.policy_id, p.owner_user_id, p.declarative_key, p.display_name,
                        p.current_revision, p.current_hash, r.policy_json, p.created_at, p.updated_at
                 FROM policies p JOIN policy_revisions r
                   ON r.policy_id = p.policy_id AND r.revision = p.current_revision
                 WHERE p.owner_user_id = ?1 AND (p.policy_id = ?2 OR p.declarative_key = ?2)",
                params![owner_user_id.to_string(), selector],
                policy_resource_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    fn load_policy_history(
        &self,
        owner_user_id: &UserId,
        selector: &str,
    ) -> Result<Vec<PolicyRevisionRecord>, StorageError> {
        let mut statement = self.conn.prepare(
            "SELECT r.policy_id, r.revision, r.payload_hash, r.policy_json,
                    r.actor, r.source, r.created_at
             FROM policy_revisions r JOIN policies p ON p.policy_id = r.policy_id
             WHERE p.owner_user_id = ?1 AND (p.policy_id = ?2 OR p.declarative_key = ?2)
             ORDER BY r.revision",
        )?;
        let rows = statement.query_map(
            params![owner_user_id.to_string(), selector],
            policy_revision_from_row,
        )?;
        collect_rows(rows)
    }

    fn load_policy_revision(
        &self,
        owner_user_id: &UserId,
        selector: &str,
        revision: u64,
    ) -> Result<Option<PolicyRevisionRecord>, StorageError> {
        self.conn
            .query_row(
                "SELECT r.policy_id, r.revision, r.payload_hash, r.policy_json,
                        r.actor, r.source, r.created_at
                 FROM policy_revisions r JOIN policies p ON p.policy_id = r.policy_id
                 WHERE p.owner_user_id = ?1 AND (p.policy_id = ?2 OR p.declarative_key = ?2)
                   AND r.revision = ?3",
                params![owner_user_id.to_string(), selector, revision],
                policy_revision_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    fn load_policy_audit(
        &self,
        owner_user_id: &UserId,
        selector: &str,
    ) -> Result<Vec<PolicyAuditRecord>, StorageError> {
        let mut statement = self.conn.prepare(
            "SELECT a.policy_id, a.actor, a.source, a.occurred_at, a.old_hash,
                    a.new_hash, a.affected_assignments_json, a.action
             FROM policy_audit_events a JOIN policies p ON p.policy_id = a.policy_id
             WHERE p.owner_user_id = ?1 AND (p.policy_id = ?2 OR p.declarative_key = ?2)
             ORDER BY a.id",
        )?;
        let rows = statement.query_map(params![owner_user_id.to_string(), selector], |row| {
            let occurred_at: String = row.get(3)?;
            let affected: String = row.get(6)?;
            let affected: PolicyAuditAssignments = serde_json::from_str(&affected)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            Ok(PolicyAuditRecord {
                policy_id: row.get(0)?,
                actor: row.get(1)?,
                source: row.get(2)?,
                occurred_at: parse_timestamp(&occurred_at)?,
                old_hash: row.get(4)?,
                new_hash: row.get(5)?,
                affected_assignments: affected.0,
                action: row.get(7)?,
            })
        })?;
        collect_rows(rows)
    }
}

fn policy_payload_hash(policy: &Policy) -> Result<String, StorageError> {
    let canonical = serde_json::to_vec(policy)?;
    Ok(format!("sha256:{:x}", Sha256::digest(canonical)))
}

fn new_public_policy_id(conn: &Connection) -> Result<String, StorageError> {
    loop {
        let candidate = format!("pol_{}", PolicyId::new().public_suffix());
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM policies WHERE policy_id = ?1)",
            params![candidate],
            |row| row.get(0),
        )?;
        if !exists {
            return Ok(candidate);
        }
    }
}

fn policy_identity_for_key(
    conn: &Connection,
    owner_user_id: &str,
    declarative_key: &str,
) -> Result<Option<(String, String)>, StorageError> {
    conn.query_row(
        "SELECT policy_id, current_hash FROM policies
         WHERE owner_user_id = ?1 AND declarative_key = ?2",
        params![owner_user_id, declarative_key],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(Into::into)
}

fn valid_declarative_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn legacy_declarative_key(authored_id: &str, payload_hash: &str) -> String {
    if valid_declarative_key(authored_id) {
        authored_id.to_string()
    } else {
        format!(
            "legacy-{}",
            &payload_hash["sha256:".len().."sha256:".len() + 16]
        )
    }
}

fn policy_resource_from_row(row: &rusqlite::Row<'_>) -> Result<PolicyResource, rusqlite::Error> {
    let owner_user_id: String = row.get(1)?;
    let policy_json: String = row.get(6)?;
    let created_at: String = row.get(7)?;
    let updated_at: String = row.get(8)?;
    Ok(PolicyResource {
        policy_id: row.get(0)?,
        owner_user_id: parse_id(&owner_user_id)?,
        declarative_key: row.get(2)?,
        display_name: row.get(3)?,
        current_revision: row.get(4)?,
        current_hash: row.get(5)?,
        policy: serde_json::from_str(&policy_json)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?,
        created_at: parse_timestamp(&created_at)?,
        updated_at: parse_timestamp(&updated_at)?,
    })
}

fn policy_revision_from_row(
    row: &rusqlite::Row<'_>,
) -> Result<PolicyRevisionRecord, rusqlite::Error> {
    let policy_json: String = row.get(3)?;
    let created_at: String = row.get(6)?;
    Ok(PolicyRevisionRecord {
        policy_id: row.get(0)?,
        revision: row.get(1)?,
        payload_hash: row.get(2)?,
        policy: serde_json::from_str(&policy_json)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?,
        actor: row.get(4)?,
        source: row.get(5)?,
        created_at: parse_timestamp(&created_at)?,
    })
}

fn table_has_column(
    conn: &Connection,
    table_name: &str,
    column_name: &str,
) -> Result<bool, StorageError> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table_name})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for row in rows {
        if row? == column_name {
            return Ok(true);
        }
    }
    Ok(false)
}

impl SpendRepository for SqliteGovernanceRepository {
    fn admit_spend_attempt(
        &mut self,
        owner_user_id: &UserId,
        operation_key: &str,
        request: &SpendRequest,
        actor: &str,
        submitted_at: DateTime<Utc>,
    ) -> Result<SpendAttemptAdmission, StorageError> {
        let request_json = serde_json::to_string(request)?;
        let transaction = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exact_revision = transaction
            .query_row(
                "SELECT revision FROM spend_operation_attempts
                 WHERE agent_id = ?1 AND operation_key = ?2 AND request_json = ?3
                 ORDER BY revision ASC LIMIT 1",
                params![request.agent_id.to_string(), operation_key, request_json],
                |row| row.get::<_, u64>(0),
            )
            .optional()?;
        if let Some(revision) = exact_revision {
            transaction.commit()?;
            return Ok(SpendAttemptAdmission::ExactReplay { revision });
        }
        let compatible_revision = {
            let mut statement = transaction.prepare(
                "SELECT revision, request_json FROM spend_operation_attempts
                 WHERE agent_id = ?1 AND operation_key = ?2
                 ORDER BY revision ASC",
            )?;
            let mut rows = statement.query(params![request.agent_id.to_string(), operation_key])?;
            let mut compatible = None;
            while let Some(row) = rows.next()? {
                let stored_json: String = row.get(1)?;
                let stored: SpendRequest = parse_json(&stored_json)?;
                if stored.replay_equivalent(request) {
                    compatible = Some(row.get(0)?);
                    break;
                }
            }
            compatible
        };
        if let Some(revision) = compatible_revision {
            transaction.commit()?;
            return Ok(SpendAttemptAdmission::ExactReplay { revision });
        }

        let unsafe_prior: bool = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM spend_operation_attempts AS attempts
                WHERE attempts.agent_id = ?1 AND attempts.operation_key = ?2
                  AND COALESCE((
                    SELECT outcomes.decision
                    FROM spend_authorization_outcomes AS outcomes
                    WHERE outcomes.agent_id = attempts.agent_id
                      AND outcomes.operation_key = attempts.operation_key
                      AND outcomes.revision = attempts.revision
                    ORDER BY outcomes.id DESC LIMIT 1
                  ), 'pending_approval') != 'denied'
             ) OR EXISTS(
                SELECT 1 FROM spend_decisions AS decisions
                JOIN spend_auth_tokens AS tokens ON tokens.spend_decision_id = decisions.id
                WHERE decisions.agent_id = ?1 AND decisions.operation_key = ?2
             ) OR EXISTS(
                SELECT 1 FROM spend_decisions AS decisions
                JOIN budget_holds AS holds ON holds.spend_decision_id = decisions.id
                WHERE decisions.agent_id = ?1 AND decisions.operation_key = ?2
             ) OR EXISTS(
                SELECT 1 FROM spend_executor_claims AS claims
                WHERE claims.agent_id = ?1 AND claims.operation_key = ?2
             )",
            params![request.agent_id.to_string(), operation_key],
            |row| row.get(0),
        )?;
        if unsafe_prior {
            transaction.commit()?;
            return Ok(SpendAttemptAdmission::ChangedScopeBlocked);
        }

        let revision: u64 = transaction.query_row(
            "SELECT COALESCE(MAX(revision), 0) + 1 FROM spend_operation_attempts
             WHERE agent_id = ?1 AND operation_key = ?2",
            params![request.agent_id.to_string(), operation_key],
            |row| row.get(0),
        )?;
        transaction.execute(
            "INSERT INTO spend_operation_attempts
             (agent_id, operation_key, revision, owner_user_id, request_json, actor, submitted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                request.agent_id.to_string(),
                operation_key,
                revision,
                owner_user_id.to_string(),
                request_json,
                actor,
                submitted_at.to_rfc3339(),
            ],
        )?;
        transaction.execute(
            "INSERT INTO spend_authorization_outcomes
             (agent_id, operation_key, revision, decision, reasons_json, created_at)
             VALUES (?1, ?2, ?3, 'pending_approval', ?4, ?5)",
            params![
                request.agent_id.to_string(),
                operation_key,
                revision,
                serde_json::to_string(&vec!["authorization evaluation in progress"])?,
                submitted_at.to_rfc3339(),
            ],
        )?;
        transaction.commit()?;
        Ok(SpendAttemptAdmission::Admitted { revision })
    }

    fn record_spend_attempt_outcome(
        &mut self,
        record: &SpendDecisionRecord,
        decision: SpendAuthorizationDecision,
        reasons: &[String],
        decided_at: DateTime<Utc>,
    ) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO spend_authorization_outcomes
             (agent_id, operation_key, revision, spend_decision_id, decision,
              reasons_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                record.request.agent_id.to_string(),
                record.operation_key,
                record.revision,
                record.id.to_string(),
                spend_authorization_decision_name(&decision),
                serde_json::to_string(reasons)?,
                decided_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    fn load_spend_attempt_history(
        &self,
        agent_id: &AgentId,
        operation_key: &str,
    ) -> Result<Vec<SpendAttemptAuditRecord>, StorageError> {
        let mut statement = self.conn.prepare(
            "SELECT attempts.revision, attempts.request_json, attempts.actor,
                    attempts.submitted_at, outcomes.spend_decision_id,
                    outcomes.decision, outcomes.created_at, outcomes.reasons_json
             FROM spend_operation_attempts AS attempts
             JOIN spend_authorization_outcomes AS outcomes ON outcomes.id = (
                SELECT latest.id FROM spend_authorization_outcomes AS latest
                WHERE latest.agent_id = attempts.agent_id
                  AND latest.operation_key = attempts.operation_key
                  AND latest.revision = attempts.revision
                ORDER BY latest.id DESC LIMIT 1
             )
             WHERE attempts.agent_id = ?1 AND attempts.operation_key = ?2
             ORDER BY attempts.revision ASC",
        )?;
        let rows = statement.query_map(params![agent_id.to_string(), operation_key], |row| {
            let request_json: String = row.get(1)?;
            let submitted_at: String = row.get(3)?;
            let decision_id: Option<String> = row.get(4)?;
            let decision: String = row.get(5)?;
            let decided_at: String = row.get(6)?;
            let reasons_json: String = row.get(7)?;
            Ok(SpendAttemptAuditRecord {
                revision: row.get(0)?,
                request: parse_json(&request_json)?,
                actor: row.get(2)?,
                submitted_at: parse_timestamp(&submitted_at)?,
                decision_id: decision_id.as_deref().map(parse_id).transpose()?,
                final_decision: parse_spend_authorization_decision(&decision)?,
                decided_at: parse_timestamp(&decided_at)?,
                reasons: parse_json(&reasons_json)?,
            })
        })?;
        collect_rows(rows)
    }

    fn changed_scope_retry_is_safe(
        &self,
        agent_id: &AgentId,
        operation_key: &str,
    ) -> Result<bool, StorageError> {
        let unsafe_prior: bool = self.conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM spend_operation_attempts AS attempts
                WHERE attempts.agent_id = ?1 AND attempts.operation_key = ?2
                  AND COALESCE((
                    SELECT outcomes.decision
                    FROM spend_authorization_outcomes AS outcomes
                    WHERE outcomes.agent_id = attempts.agent_id
                      AND outcomes.operation_key = attempts.operation_key
                      AND outcomes.revision = attempts.revision
                    ORDER BY outcomes.id DESC LIMIT 1
                  ), 'pending_approval') != 'denied'
             ) OR EXISTS(
                SELECT 1 FROM spend_decisions AS decisions
                JOIN spend_auth_tokens AS tokens ON tokens.spend_decision_id = decisions.id
                WHERE decisions.agent_id = ?1 AND decisions.operation_key = ?2
             ) OR EXISTS(
                SELECT 1 FROM spend_decisions AS decisions
                JOIN budget_holds AS holds ON holds.spend_decision_id = decisions.id
                WHERE decisions.agent_id = ?1 AND decisions.operation_key = ?2
             ) OR EXISTS(
                SELECT 1 FROM spend_executor_claims AS claims
                WHERE claims.agent_id = ?1 AND claims.operation_key = ?2
             )",
            params![agent_id.to_string(), operation_key],
            |row| row.get(0),
        )?;
        Ok(!unsafe_prior)
    }

    fn save_spend_decision(&mut self, record: &SpendDecisionRecord) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO spend_decisions
             (id, owner_user_id, agent_id, operation_key, revision, actor,
              request_json, evaluation_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                record.id.to_string(),
                record.owner_user_id.to_string(),
                record.request.agent_id.to_string(),
                record.operation_key,
                record.revision,
                record.actor,
                serde_json::to_string(&record.request)?,
                serde_json::to_string(&record.evaluation)?,
                record.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    fn save_spend_auth_token(&mut self, record: &SpendAuthTokenRecord) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO spend_auth_tokens
             (id, owner_user_id, spend_decision_id, expires_at, claim_ttl_seconds,
              used_at, used_by_payment_id, revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                record.id.to_string(),
                record.owner_user_id.to_string(),
                record.spend_decision_id.to_string(),
                record.expires_at.to_rfc3339(),
                record.claim_ttl_seconds,
                record.used_at.map(|timestamp| timestamp.to_rfc3339()),
                record.used_by_payment_id.as_ref().map(ToString::to_string),
                record.revoked_at.map(|timestamp| timestamp.to_rfc3339()),
            ],
        )?;
        Ok(())
    }

    fn update_spend_auth_token(
        &mut self,
        record: &SpendAuthTokenRecord,
    ) -> Result<(), StorageError> {
        self.conn.execute(
            "UPDATE spend_auth_tokens
             SET used_at = ?2, used_by_payment_id = ?3, revoked_at = ?4
             WHERE id = ?1",
            params![
                record.id.to_string(),
                record.used_at.map(|timestamp| timestamp.to_rfc3339()),
                record.used_by_payment_id.as_ref().map(ToString::to_string),
                record.revoked_at.map(|timestamp| timestamp.to_rfc3339()),
            ],
        )?;
        Ok(())
    }

    fn load_spend_decisions(&self) -> Result<Vec<SpendDecisionRecord>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, owner_user_id, agent_id, operation_key, revision, actor,
                    request_json, evaluation_json, created_at
             FROM spend_decisions
             ORDER BY agent_id, operation_key, revision ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let owner_user_id: String = row.get(1)?;
            let agent_id: String = row.get(2)?;
            let operation_key: String = row.get(3)?;
            let request_json: String = row.get(6)?;
            let evaluation_json: String = row.get(7)?;
            let created_at: String = row.get(8)?;
            let request: crate::spend::SpendRequest = parse_json(&request_json)?;
            if request.agent_id.to_string() != agent_id {
                return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(
                    StorageError::InvalidData(
                        "spend decision agent id does not match request".to_string(),
                    ),
                )));
            }
            Ok(SpendDecisionRecord {
                id: parse_id(&id)?,
                owner_user_id: parse_id(&owner_user_id)?,
                operation_key,
                revision: row.get(4)?,
                actor: row.get(5)?,
                request,
                evaluation: parse_json(&evaluation_json)?,
                created_at: parse_timestamp(&created_at)?,
            })
        })?;
        collect_rows(rows)
    }

    fn load_spend_auth_tokens(&self) -> Result<Vec<SpendAuthTokenRecord>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, owner_user_id, spend_decision_id, expires_at, claim_ttl_seconds,
                    used_at, used_by_payment_id, revoked_at
             FROM spend_auth_tokens
             ORDER BY expires_at ASC",
        )?;
        let rows = stmt.query_map([], spend_auth_token_from_row)?;
        collect_rows(rows)
    }

    fn save_executor_claim(
        &mut self,
        record: &SpendExecutorClaimRecord,
    ) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO spend_executor_claims
             (id, spend_auth_token_id, owner_user_id, agent_id, operation_key,
              workload_profile, status, claimed_at, expires_at, finalized_at, settlement_id,
              provider_reference, reconciliation_evidence, reconciled_at, reconciled_by_user_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(id) DO NOTHING",
            params![
                record.id.to_string(),
                record.spend_auth_token_id.to_string(),
                record.owner_user_id.to_string(),
                record.agent_id.to_string(),
                record.operation_key,
                record.workload_profile,
                executor_claim_status(&record.status),
                record.claimed_at.to_rfc3339(),
                record.expires_at.to_rfc3339(),
                record.finalized_at.map(|timestamp| timestamp.to_rfc3339()),
                record.settlement_id.as_ref().map(ToString::to_string),
                record.provider_reference,
                record.reconciliation_evidence,
                record.reconciled_at.map(|timestamp| timestamp.to_rfc3339()),
                record
                    .reconciled_by_user_id
                    .as_ref()
                    .map(ToString::to_string),
            ],
        )?;
        Ok(())
    }

    fn load_executor_claims(&self) -> Result<Vec<SpendExecutorClaimRecord>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, spend_auth_token_id, owner_user_id, agent_id, operation_key,
                    workload_profile, status, claimed_at, expires_at, finalized_at, settlement_id,
                    provider_reference, reconciliation_evidence, reconciled_at,
                    reconciled_by_user_id
             FROM spend_executor_claims
             ORDER BY claimed_at ASC",
        )?;
        let rows = stmt.query_map([], executor_claim_from_row)?;
        collect_rows(rows)
    }
}

impl BudgetRepository for SqliteGovernanceRepository {
    fn expire_overdue_budget_holds(&mut self, now: DateTime<Utc>) -> Result<(), StorageError> {
        let sqlite_tx = self.conn.transaction()?;
        let expired_holds = {
            let mut stmt = sqlite_tx.prepare(
                "SELECT id, budget_id, amount_cents
                 FROM budget_holds
                 WHERE status = 'frozen' AND expires_at <= ?1
                 ORDER BY created_at ASC",
            )?;
            let rows = stmt.query_map(params![now.to_rfc3339()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        for (hold_id, budget_id, amount_cents) in expired_holds {
            sqlite_tx.execute(
                "UPDATE budget_holds
                 SET status = 'expired', updated_at = ?2
                 WHERE id = ?1 AND status = 'frozen'",
                params![hold_id, now.to_rfc3339()],
            )?;
            sqlite_tx.execute(
                "UPDATE budget_balances
                 SET frozen_amount_cents = frozen_amount_cents - ?2,
                     remaining_amount_cents = remaining_amount_cents + ?2,
                     updated_at = ?3
                 WHERE budget_id = ?1",
                params![budget_id, amount_cents, now.to_rfc3339()],
            )?;
            refresh_persisted_budget_status(&sqlite_tx, &budget_id, now)?;
        }

        sqlite_tx.commit()?;
        Ok(())
    }

    fn save_budget_with_balance(
        &mut self,
        budget: &Budget,
        balance: &BudgetBalance,
    ) -> Result<(), StorageError> {
        // Keep the legacy scope columns as a storage-compatibility seam, but the
        // MVP domain accepts and writes only agent-owned budgets.
        let sqlite_tx = self.conn.transaction()?;
        sqlite_tx.execute(
            "INSERT INTO budgets
             (id, scope_type, scope_id, amount_limit_cents, currency, starting_at, ending_before, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(id) DO UPDATE SET
                scope_type = excluded.scope_type,
                scope_id = excluded.scope_id,
                amount_limit_cents = excluded.amount_limit_cents,
                currency = excluded.currency,
                starting_at = excluded.starting_at,
                ending_before = excluded.ending_before,
                status = excluded.status,
                updated_at = excluded.updated_at",
            params![
                budget.id.to_string(),
                "agent",
                budget.agent_id.to_string(),
                budget.amount_limit_cents,
                budget.currency.to_string(),
                budget.period.starting_at.to_rfc3339(),
                budget.period.ending_before.map(|timestamp| timestamp.to_rfc3339()),
                budget_status(&budget.status),
                budget.created_at.to_rfc3339(),
                budget.updated_at.to_rfc3339(),
            ],
        )?;
        upsert_balance(&sqlite_tx, balance)?;
        sqlite_tx.commit()?;
        Ok(())
    }

    fn save_budget_hold(
        &mut self,
        hold: &BudgetHold,
        balance: &BudgetBalance,
    ) -> Result<(), StorageError> {
        let sqlite_tx = self.conn.transaction()?;
        sqlite_tx.execute(
            "INSERT INTO budget_holds
             (id, budget_id, spend_decision_id, amount_cents, currency, status, executor_claim_id,
              created_at, updated_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                hold.id.to_string(),
                hold.budget_id.to_string(),
                hold.spend_decision_id.to_string(),
                hold.amount_cents,
                hold.currency.to_string(),
                budget_hold_status(&hold.status),
                hold.executor_claim_id.as_ref().map(ToString::to_string),
                hold.created_at.to_rfc3339(),
                hold.updated_at.to_rfc3339(),
                hold.expires_at.to_rfc3339(),
            ],
        )?;
        upsert_balance(&sqlite_tx, balance)?;
        refresh_persisted_budget_status(&sqlite_tx, &hold.budget_id.to_string(), hold.updated_at)?;
        sqlite_tx.commit()?;
        Ok(())
    }

    fn update_budget_hold(
        &mut self,
        hold: &BudgetHold,
        balance: &BudgetBalance,
    ) -> Result<(), StorageError> {
        let sqlite_tx = self.conn.transaction()?;
        sqlite_tx.execute(
            "UPDATE budget_holds
             SET status = ?2, executor_claim_id = ?3, updated_at = ?4, expires_at = ?5
             WHERE id = ?1",
            params![
                hold.id.to_string(),
                budget_hold_status(&hold.status),
                hold.executor_claim_id.as_ref().map(ToString::to_string),
                hold.updated_at.to_rfc3339(),
                hold.expires_at.to_rfc3339(),
            ],
        )?;
        upsert_balance(&sqlite_tx, balance)?;
        refresh_persisted_budget_status(&sqlite_tx, &hold.budget_id.to_string(), hold.updated_at)?;
        sqlite_tx.commit()?;
        Ok(())
    }

    fn load_budgets(&self) -> Result<Vec<Budget>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, scope_type, scope_id, amount_limit_cents, currency, starting_at,
                    ending_before, status, created_at, updated_at
             FROM budgets
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let scope_type: String = row.get(1)?;
            let scope_id: String = row.get(2)?;
            let currency: String = row.get(4)?;
            let starting_at: String = row.get(5)?;
            let ending_before: Option<String> = row.get(6)?;
            let status: String = row.get(7)?;
            let created_at: String = row.get(8)?;
            let updated_at: String = row.get(9)?;
            Ok(Budget {
                id: parse_id(&id)?,
                agent_id: parse_budget_agent_id(&scope_type, &scope_id)?,
                amount_limit_cents: row.get(3)?,
                currency: parse_currency(&currency)?,
                period: TimePeriod::new(
                    parse_timestamp(&starting_at)?,
                    parse_optional_timestamp(ending_before)?,
                )
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                status: parse_budget_status(&status)?,
                created_at: parse_timestamp(&created_at)?,
                updated_at: parse_timestamp(&updated_at)?,
            })
        })?;
        collect_rows(rows)
    }

    fn load_budget_balances(&self) -> Result<Vec<BudgetBalance>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT budget_id, consumed_amount_cents, frozen_amount_cents, remaining_amount_cents
             FROM budget_balances
             ORDER BY budget_id ASC",
        )?;
        let rows = stmt.query_map([], budget_balance_from_row)?;
        collect_rows(rows)
    }

    fn load_budget_holds(&self) -> Result<Vec<BudgetHold>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, budget_id, spend_decision_id, amount_cents, currency, status,
                    executor_claim_id, created_at, updated_at, expires_at
             FROM budget_holds
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], budget_hold_from_row)?;
        collect_rows(rows)
    }
}

impl SpendingTargetRepository for SqliteGovernanceRepository {
    fn save_spending_target(&mut self, target: &SpendingTarget) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO spending_targets
             (id, owner_user_id, target_amount_cents, currency, starting_at,
              ending_before, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
                owner_user_id = excluded.owner_user_id,
                target_amount_cents = excluded.target_amount_cents,
                currency = excluded.currency,
                starting_at = excluded.starting_at,
                ending_before = excluded.ending_before,
                status = excluded.status,
                updated_at = excluded.updated_at",
            params![
                target.id.to_string(),
                target.owner_user_id.to_string(),
                target.target_amount_cents,
                target.currency.to_string(),
                target.period.starting_at.to_rfc3339(),
                target
                    .period
                    .ending_before
                    .map(|timestamp| timestamp.to_rfc3339()),
                spending_target_status(target.status),
                target.created_at.to_rfc3339(),
                target.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    fn load_spending_targets(&self) -> Result<Vec<SpendingTarget>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, owner_user_id, target_amount_cents, currency, starting_at,
                    ending_before, status, created_at, updated_at
             FROM spending_targets
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let owner_user_id: String = row.get(1)?;
            let currency: String = row.get(3)?;
            let starting_at: String = row.get(4)?;
            let ending_before: Option<String> = row.get(5)?;
            let status: String = row.get(6)?;
            let created_at: String = row.get(7)?;
            let updated_at: String = row.get(8)?;
            Ok(SpendingTarget {
                id: parse_id(&id)?,
                owner_user_id: parse_id(&owner_user_id)?,
                target_amount_cents: row.get(2)?,
                currency: parse_currency(&currency)?,
                period: TimePeriod::new(
                    parse_timestamp(&starting_at)?,
                    parse_optional_timestamp(ending_before)?,
                )
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                status: parse_spending_target_status(&status)?,
                created_at: parse_timestamp(&created_at)?,
                updated_at: parse_timestamp(&updated_at)?,
            })
        })?;
        collect_rows(rows)
    }
}

fn load_executor_claim_by_id(
    conn: &Connection,
    claim_id: &SpendExecutorClaimId,
) -> Result<Option<SpendExecutorClaimRecord>, StorageError> {
    conn.query_row(
        "SELECT id, spend_auth_token_id, owner_user_id, agent_id, operation_key,
                workload_profile, status, claimed_at, expires_at, finalized_at, settlement_id,
                provider_reference, reconciliation_evidence, reconciled_at,
                reconciled_by_user_id
         FROM spend_executor_claims
         WHERE id = ?1",
        params![claim_id.to_string()],
        executor_claim_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn load_executor_claim_by_operation(
    conn: &Connection,
    agent_id: &AgentId,
    operation_key: &str,
) -> Result<Option<SpendExecutorClaimRecord>, StorageError> {
    conn.query_row(
        "SELECT id, spend_auth_token_id, owner_user_id, agent_id, operation_key,
                workload_profile, status, claimed_at, expires_at, finalized_at, settlement_id,
                provider_reference, reconciliation_evidence, reconciled_at,
                reconciled_by_user_id
         FROM spend_executor_claims
         WHERE agent_id = ?1 AND operation_key = ?2",
        params![agent_id.to_string(), operation_key],
        executor_claim_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn load_executor_settlement_receipt_by_claim_id(
    conn: &Connection,
    claim_id: &SpendExecutorClaimId,
) -> Result<Option<PersistedSpendExecutorSettlementReceipt>, StorageError> {
    conn.query_row(
        "SELECT claim_id, settlement_id, authorized_max_cents, actual_vendor_cost_cents,
                released_amount_cents, currency, provider_request_id,
                price_model_snapshot_json, artifact_reference, created_at
         FROM spend_executor_settlement_receipts
         WHERE claim_id = ?1",
        params![claim_id.to_string()],
        executor_settlement_receipt_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn save_executor_settlement_receipt(
    conn: &Connection,
    record: &PersistedSpendExecutorSettlementReceipt,
) -> Result<(), StorageError> {
    let price_model_snapshot_json = serde_json::to_string(&record.receipt.price_model_snapshot)?;
    conn.execute(
        "INSERT INTO spend_executor_settlement_receipts
         (claim_id, settlement_id, authorized_max_cents, actual_vendor_cost_cents,
          released_amount_cents, currency, provider_request_id, price_model_snapshot_json,
          artifact_reference, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            record.claim_id.to_string(),
            record.settlement_id.to_string(),
            record.authorized_max_cents,
            record.receipt.actual_vendor_cost_cents,
            record.released_amount_cents,
            record.currency.to_string(),
            record.receipt.provider_request_id,
            price_model_snapshot_json,
            record.receipt.artifact_reference,
            record.created_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

fn validate_executor_settlement_receipt(
    receipt: &SpendExecutorSettlementReceipt,
    hold: &BudgetHold,
) -> Result<(), StorageError> {
    if receipt.actual_vendor_cost_cents < 0 {
        return Err(StorageError::InvalidData(
            "actual vendor cost cannot be negative".to_string(),
        ));
    }
    if receipt.actual_vendor_cost_cents > hold.amount_cents {
        return Err(StorageError::InvalidData(
            "actual vendor cost exceeds the authorized maximum".to_string(),
        ));
    }
    if receipt.provider_request_id.trim().is_empty() {
        return Err(StorageError::InvalidData(
            "provider request id cannot be empty".to_string(),
        ));
    }
    if receipt.artifact_reference.trim().is_empty() {
        return Err(StorageError::InvalidData(
            "artifact reference cannot be empty".to_string(),
        ));
    }
    let snapshot = &receipt.price_model_snapshot;
    if snapshot.provider.trim().is_empty() {
        return Err(StorageError::InvalidData(
            "price/model snapshot provider cannot be empty".to_string(),
        ));
    }
    if snapshot.model.trim().is_empty() {
        return Err(StorageError::InvalidData(
            "price/model snapshot model cannot be empty".to_string(),
        ));
    }
    if snapshot.unit_price_cents < 0 {
        return Err(StorageError::InvalidData(
            "price/model snapshot unit price cannot be negative".to_string(),
        ));
    }
    if snapshot.pricing_unit.trim().is_empty() {
        return Err(StorageError::InvalidData(
            "price/model snapshot pricing unit cannot be empty".to_string(),
        ));
    }
    if snapshot.currency != hold.currency {
        return Err(StorageError::InvalidData(
            "price/model snapshot currency does not match the authorization".to_string(),
        ));
    }
    Ok(())
}

fn load_spend_auth_token_by_id(
    conn: &Connection,
    token_id: &hubu_common::ids::SpendAuthTokenId,
) -> Result<Option<SpendAuthTokenRecord>, StorageError> {
    conn.query_row(
        "SELECT id, owner_user_id, spend_decision_id, expires_at, claim_ttl_seconds,
                used_at, used_by_payment_id, revoked_at
         FROM spend_auth_tokens
         WHERE id = ?1",
        params![token_id.to_string()],
        spend_auth_token_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn load_budget_hold_by_claim_id(
    conn: &Connection,
    claim_id: &SpendExecutorClaimId,
) -> Result<Option<BudgetHold>, StorageError> {
    conn.query_row(
        "SELECT id, budget_id, spend_decision_id, amount_cents, currency, status,
                executor_claim_id, created_at, updated_at, expires_at
         FROM budget_holds
         WHERE executor_claim_id = ?1",
        params![claim_id.to_string()],
        budget_hold_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn load_budget_balance_by_id(
    conn: &Connection,
    budget_id: &hubu_common::ids::BudgetId,
) -> Result<Option<BudgetBalance>, StorageError> {
    conn.query_row(
        "SELECT budget_id, consumed_amount_cents, frozen_amount_cents, remaining_amount_cents
         FROM budget_balances
         WHERE budget_id = ?1",
        params![budget_id.to_string()],
        budget_balance_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn executor_claim_from_row(
    row: &rusqlite::Row<'_>,
) -> Result<SpendExecutorClaimRecord, rusqlite::Error> {
    let id: String = row.get(0)?;
    let token_id: String = row.get(1)?;
    let owner_user_id: String = row.get(2)?;
    let agent_id: String = row.get(3)?;
    let status: String = row.get(6)?;
    let claimed_at: String = row.get(7)?;
    let expires_at: String = row.get(8)?;
    let finalized_at: Option<String> = row.get(9)?;
    let settlement_id: Option<String> = row.get(10)?;
    let reconciled_at: Option<String> = row.get(13)?;
    let reconciled_by_user_id: Option<String> = row.get(14)?;
    Ok(SpendExecutorClaimRecord {
        id: parse_id(&id)?,
        spend_auth_token_id: parse_id(&token_id)?,
        owner_user_id: parse_id(&owner_user_id)?,
        agent_id: parse_id(&agent_id)?,
        operation_key: row.get(4)?,
        workload_profile: row.get(5)?,
        status: parse_executor_claim_status(&status)?,
        claimed_at: parse_timestamp(&claimed_at)?,
        expires_at: parse_timestamp(&expires_at)?,
        finalized_at: parse_optional_timestamp(finalized_at)?,
        settlement_id: parse_optional_id(settlement_id)?,
        provider_reference: row.get(11)?,
        reconciliation_evidence: row.get(12)?,
        reconciled_at: parse_optional_timestamp(reconciled_at)?,
        reconciled_by_user_id: parse_optional_id(reconciled_by_user_id)?,
    })
}

fn executor_settlement_receipt_from_row(
    row: &rusqlite::Row<'_>,
) -> Result<PersistedSpendExecutorSettlementReceipt, rusqlite::Error> {
    let claim_id: String = row.get(0)?;
    let settlement_id: String = row.get(1)?;
    let currency: String = row.get(5)?;
    let price_model_snapshot_json: String = row.get(7)?;
    let created_at: String = row.get(9)?;
    Ok(PersistedSpendExecutorSettlementReceipt {
        claim_id: parse_id(&claim_id)?,
        settlement_id: parse_id(&settlement_id)?,
        authorized_max_cents: row.get(2)?,
        released_amount_cents: row.get(4)?,
        currency: parse_currency(&currency)?,
        receipt: SpendExecutorSettlementReceipt {
            actual_vendor_cost_cents: row.get(3)?,
            provider_request_id: row.get(6)?,
            price_model_snapshot: parse_json(&price_model_snapshot_json)?,
            artifact_reference: row.get(8)?,
        },
        created_at: parse_timestamp(&created_at)?,
    })
}

fn spend_auth_token_from_row(
    row: &rusqlite::Row<'_>,
) -> Result<SpendAuthTokenRecord, rusqlite::Error> {
    let id: String = row.get(0)?;
    let owner_user_id: String = row.get(1)?;
    let spend_decision_id: String = row.get(2)?;
    let expires_at: String = row.get(3)?;
    let used_at: Option<String> = row.get(5)?;
    let used_by_payment_id: Option<String> = row.get(6)?;
    let revoked_at: Option<String> = row.get(7)?;
    Ok(SpendAuthTokenRecord {
        id: parse_id(&id)?,
        owner_user_id: parse_id(&owner_user_id)?,
        spend_decision_id: parse_id(&spend_decision_id)?,
        expires_at: parse_timestamp(&expires_at)?,
        claim_ttl_seconds: row.get(4)?,
        used_at: parse_optional_timestamp(used_at)?,
        used_by_payment_id: parse_optional_id(used_by_payment_id)?,
        revoked_at: parse_optional_timestamp(revoked_at)?,
    })
}

fn budget_hold_from_row(row: &rusqlite::Row<'_>) -> Result<BudgetHold, rusqlite::Error> {
    let id: String = row.get(0)?;
    let budget_id: String = row.get(1)?;
    let spend_decision_id: String = row.get(2)?;
    let currency: String = row.get(4)?;
    let status: String = row.get(5)?;
    let executor_claim_id: Option<String> = row.get(6)?;
    let created_at: String = row.get(7)?;
    let updated_at: String = row.get(8)?;
    let expires_at: String = row.get(9)?;
    Ok(BudgetHold {
        id: parse_id(&id)?,
        budget_id: parse_id(&budget_id)?,
        spend_decision_id: parse_id(&spend_decision_id)?,
        amount_cents: row.get(3)?,
        currency: parse_currency(&currency)?,
        status: parse_budget_hold_status(&status)?,
        executor_claim_id: parse_optional_id(executor_claim_id)?,
        created_at: parse_timestamp(&created_at)?,
        updated_at: parse_timestamp(&updated_at)?,
        expires_at: parse_timestamp(&expires_at)?,
    })
}

fn budget_balance_from_row(row: &rusqlite::Row<'_>) -> Result<BudgetBalance, rusqlite::Error> {
    let budget_id: String = row.get(0)?;
    Ok(BudgetBalance {
        budget_id: parse_id(&budget_id)?,
        consumed_amount_cents: row.get(1)?,
        frozen_amount_cents: row.get(2)?,
        remaining_amount_cents: row.get(3)?,
    })
}

fn require_one_updated_row(rows: usize, message: &str) -> Result<(), StorageError> {
    if rows == 1 {
        Ok(())
    } else {
        Err(StorageError::InvalidData(message.to_string()))
    }
}

fn upsert_balance(conn: &Connection, balance: &BudgetBalance) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO budget_balances
         (budget_id, consumed_amount_cents, frozen_amount_cents, remaining_amount_cents, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(budget_id) DO UPDATE SET
            consumed_amount_cents = excluded.consumed_amount_cents,
            frozen_amount_cents = excluded.frozen_amount_cents,
            remaining_amount_cents = excluded.remaining_amount_cents,
            updated_at = excluded.updated_at",
        params![
            balance.budget_id.to_string(),
            balance.consumed_amount_cents,
            balance.frozen_amount_cents,
            balance.remaining_amount_cents,
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

fn refresh_persisted_budget_status(
    conn: &Connection,
    budget_id: &str,
    now: DateTime<Utc>,
) -> Result<(), rusqlite::Error> {
    let now = now.to_rfc3339();
    conn.execute(
        "UPDATE budgets
         SET status = 'exhausted', updated_at = ?2
         WHERE id = ?1
           AND status = 'active'
           AND EXISTS (
             SELECT 1 FROM budget_balances
             WHERE budget_id = ?1
               AND remaining_amount_cents = 0
               AND frozen_amount_cents = 0
           )",
        params![budget_id, now],
    )?;
    conn.execute(
        "UPDATE budgets
         SET status = 'active', updated_at = ?2
         WHERE id = ?1
           AND status = 'exhausted'
           AND EXISTS (
             SELECT 1 FROM budget_balances
             WHERE budget_id = ?1
               AND remaining_amount_cents > 0
           )",
        params![budget_id, now],
    )?;
    Ok(())
}

fn budget_status(status: &BudgetStatus) -> &'static str {
    match status {
        BudgetStatus::Active => "active",
        BudgetStatus::Exhausted => "exhausted",
        BudgetStatus::Expired => "expired",
        BudgetStatus::Revoked => "revoked",
    }
}

fn budget_hold_status(status: &BudgetHoldStatus) -> &'static str {
    match status {
        BudgetHoldStatus::Frozen => "frozen",
        BudgetHoldStatus::Claimed => "claimed",
        BudgetHoldStatus::Settled => "settled",
        BudgetHoldStatus::Released => "released",
        BudgetHoldStatus::Expired => "expired",
    }
}

fn executor_claim_status(status: &SpendExecutorClaimStatus) -> &'static str {
    match status {
        SpendExecutorClaimStatus::Claimed => "claimed",
        SpendExecutorClaimStatus::Settled => "settled",
        SpendExecutorClaimStatus::Released => "released",
    }
}

fn spend_authorization_decision_name(decision: &SpendAuthorizationDecision) -> &'static str {
    match decision {
        SpendAuthorizationDecision::PendingApproval => "pending_approval",
        SpendAuthorizationDecision::Denied => "denied",
        SpendAuthorizationDecision::Allowed => "allowed",
    }
}

fn parse_spend_authorization_decision(
    value: &str,
) -> Result<SpendAuthorizationDecision, rusqlite::Error> {
    match value {
        "pending_approval" => Ok(SpendAuthorizationDecision::PendingApproval),
        "denied" => Ok(SpendAuthorizationDecision::Denied),
        "allowed" => Ok(SpendAuthorizationDecision::Allowed),
        other => Err(rusqlite::Error::ToSqlConversionFailure(Box::new(
            StorageError::InvalidData(format!("unknown spend authorization decision `{other}`")),
        ))),
    }
}

fn spending_target_status(status: SpendingTargetStatus) -> &'static str {
    match status {
        SpendingTargetStatus::Active => "active",
        SpendingTargetStatus::Revoked => "revoked",
    }
}

fn parse_budget_agent_id(scope_type: &str, scope_id: &str) -> Result<AgentId, rusqlite::Error> {
    match scope_type {
        "agent" => parse_id(scope_id),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_budget_status(value: &str) -> Result<BudgetStatus, rusqlite::Error> {
    match value {
        "active" => Ok(BudgetStatus::Active),
        "exhausted" => Ok(BudgetStatus::Exhausted),
        "expired" => Ok(BudgetStatus::Expired),
        "revoked" => Ok(BudgetStatus::Revoked),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_budget_hold_status(value: &str) -> Result<BudgetHoldStatus, rusqlite::Error> {
    match value {
        "frozen" => Ok(BudgetHoldStatus::Frozen),
        "claimed" => Ok(BudgetHoldStatus::Claimed),
        "settled" => Ok(BudgetHoldStatus::Settled),
        "released" => Ok(BudgetHoldStatus::Released),
        "expired" => Ok(BudgetHoldStatus::Expired),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_executor_claim_status(value: &str) -> Result<SpendExecutorClaimStatus, rusqlite::Error> {
    match value {
        "claimed" => Ok(SpendExecutorClaimStatus::Claimed),
        "settled" => Ok(SpendExecutorClaimStatus::Settled),
        "released" => Ok(SpendExecutorClaimStatus::Released),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_spending_target_status(value: &str) -> Result<SpendingTargetStatus, rusqlite::Error> {
    match value {
        "active" => Ok(SpendingTargetStatus::Active),
        "revoked" => Ok(SpendingTargetStatus::Revoked),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_timestamp(value: &str) -> Result<DateTime<Utc>, rusqlite::Error> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|_| rusqlite::Error::InvalidQuery)
}

fn parse_optional_timestamp(
    value: Option<String>,
) -> Result<Option<DateTime<Utc>>, rusqlite::Error> {
    value.map(|value| parse_timestamp(&value)).transpose()
}

fn parse_currency(value: &str) -> Result<Currency, rusqlite::Error> {
    Currency::from_str(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn parse_id<T>(value: &str) -> Result<T, rusqlite::Error>
where
    T: FromStr,
{
    T::from_str(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn parse_optional_id<T>(value: Option<String>) -> Result<Option<T>, rusqlite::Error>
where
    T: FromStr,
{
    value.as_deref().map(parse_id).transpose()
}

fn parse_json<T>(value: &str) -> Result<T, rusqlite::Error>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_str(value)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
}

fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> Result<T, rusqlite::Error>>,
) -> Result<Vec<T>, StorageError> {
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use chrono::Duration;
    use hubu_common::ids::{
        AgentAccountId, BudgetHoldId, BudgetId, SpendAuthTokenId, SpendDecisionId,
        SpendExecutorClaimId, SpendingTargetId,
    };

    use super::*;
    use crate::policy::{
        condition::{Condition, Field, PolicyValue},
        Effect, Evaluation, Rule, RuleResult,
    };
    use crate::spend::{SpendExecutorPriceModelSnapshot, SpendRequest};

    fn user_id() -> UserId {
        "00000000-0000-4000-8000-000000000123".parse().unwrap()
    }

    fn agent_id() -> AgentId {
        "00000000-0000-4000-8000-000000000456".parse().unwrap()
    }

    fn settlement_receipt(actual_vendor_cost_cents: i64) -> SpendExecutorSettlementReceipt {
        SpendExecutorSettlementReceipt {
            actual_vendor_cost_cents,
            provider_request_id: "provider-request-123".to_string(),
            price_model_snapshot: SpendExecutorPriceModelSnapshot {
                provider: "example-image-provider".to_string(),
                model: "image-model-v1".to_string(),
                unit_price_cents: actual_vendor_cost_cents,
                pricing_unit: "image".to_string(),
                currency: Currency::Usd,
            },
            artifact_reference: "artifact://hubu-logo.png".to_string(),
        }
    }

    fn policy() -> Policy {
        Policy {
            id: "demo_policy".to_string(),
            version: "v1".to_string(),
            owner_user_id: user_id(),
            default_effect: Effect::NeedsApproval,
            rules: vec![Rule {
                id: "allow_small".to_string(),
                effect: Effect::Allow,
                reason: "small spend".to_string(),
                when: Condition::Lte {
                    field: Field::Amount,
                    value: PolicyValue::MoneyCents(5_000),
                },
            }],
        }
    }

    fn spend_request() -> SpendRequest {
        SpendRequest {
            amount_cents: 2_500,
            currency: Currency::Usd,
            owner_user_id: user_id(),
            agent_id: agent_id(),
            agent_account_id: AgentAccountId::new(),
            merchant: Some("Acme".to_string()),
            execution_scope: None,
            category: None,
            task_id: Some("task".to_string()),
            workload_profile: "default".to_string(),
        }
    }

    fn legacy_execution_scope(merchant: &str) -> hubu_common::execution_scope::ExecutionScope {
        use hubu_common::execution_scope::{ExecutionScope, ScopeIdentity};

        let merchant = merchant.trim();
        let digest = format!("{:x}", Sha256::digest(merchant.as_bytes()));
        ExecutionScope {
            schema_version: hubu_common::execution_scope::EXECUTION_SCOPE_SCHEMA_VERSION,
            provider: ScopeIdentity {
                id: "provider:legacy:unresolved".into(),
                display_name: "Legacy unresolved provider".into(),
            },
            executor: ScopeIdentity {
                id: "executor:legacy:unresolved".into(),
                display_name: "Legacy unresolved executor".into(),
            },
            capability: ScopeIdentity {
                id: "capability:legacy:unresolved".into(),
                display_name: "Legacy unresolved capability".into(),
            },
            billing_merchant: ScopeIdentity {
                id: format!("merchant:legacy:{}", &digest[..16]),
                display_name: merchant.into(),
            },
        }
    }

    fn spend_decision() -> SpendDecisionRecord {
        SpendDecisionRecord {
            id: SpendDecisionId::new(),
            owner_user_id: user_id(),
            operation_key: "gongbu-operation-transactional".to_string(),
            revision: 1,
            actor: "test:actor".to_string(),
            request: spend_request(),
            evaluation: Evaluation {
                policy_id: "demo_policy".to_string(),
                policy_version: "v1".to_string(),
                decision: Effect::Allow,
                reasons: vec!["small spend".to_string()],
                rule_results: vec![RuleResult {
                    rule_id: "allow_small".to_string(),
                    matched: true,
                    effect: Some(Effect::Allow),
                    reason: Some("small spend".to_string()),
                }],
            },
            created_at: Utc::now(),
        }
    }

    fn denied_spend_decision(revision: u64, request: SpendRequest) -> SpendDecisionRecord {
        let mut decision = spend_decision();
        decision.revision = revision;
        decision.request = request;
        decision.evaluation.decision = Effect::Deny;
        decision.evaluation.reasons = vec!["policy denied spend".to_string()];
        decision
    }

    fn persist_claimed_executor_spend(
        repo: &mut SqliteGovernanceRepository,
        claim_expires_at: DateTime<Utc>,
    ) -> (SpendExecutorClaimRecord, SpendAuthTokenRecord, BudgetHold) {
        let decision = spend_decision();
        let token = SpendAuthTokenRecord {
            id: SpendAuthTokenId::new(),
            owner_user_id: user_id(),
            spend_decision_id: decision.id.clone(),
            expires_at: Utc::now() + Duration::minutes(5),
            claim_ttl_seconds: 900,
            used_at: None,
            used_by_payment_id: None,
            revoked_at: None,
        };
        let claim = SpendExecutorClaimRecord {
            id: SpendExecutorClaimId::new(),
            spend_auth_token_id: token.id.clone(),
            owner_user_id: user_id(),
            agent_id: decision.request.agent_id.clone(),
            operation_key: decision.operation_key.clone(),
            workload_profile: "default".to_string(),
            status: SpendExecutorClaimStatus::Claimed,
            claimed_at: Utc::now(),
            expires_at: claim_expires_at,
            finalized_at: None,
            settlement_id: None,
            provider_reference: None,
            reconciliation_evidence: None,
            reconciled_at: None,
            reconciled_by_user_id: None,
        };
        let budget = Budget::new(
            BudgetId::new(),
            agent_id(),
            10_000,
            Currency::Usd,
            TimePeriod::new(
                Utc::now() - Duration::hours(1),
                Some(Utc::now() + Duration::hours(1)),
            )
            .unwrap(),
        )
        .unwrap();
        let balance = BudgetBalance {
            budget_id: budget.id.clone(),
            consumed_amount_cents: 0,
            frozen_amount_cents: 2_500,
            remaining_amount_cents: 7_500,
        };
        let hold = BudgetHold {
            id: BudgetHoldId::new(),
            budget_id: budget.id.clone(),
            spend_decision_id: decision.id.clone(),
            amount_cents: 2_500,
            currency: Currency::Usd,
            status: BudgetHoldStatus::Claimed,
            executor_claim_id: Some(claim.id.clone()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            expires_at: claim_expires_at,
        };

        repo.save_spend_decision(&decision).unwrap();
        repo.save_spend_auth_token(&token).unwrap();
        repo.save_executor_claim(&claim).unwrap();
        repo.save_budget_with_balance(&budget, &balance).unwrap();
        repo.save_budget_hold(&hold, &balance).unwrap();
        (claim, token, hold)
    }

    #[test]
    fn migrates_executor_claim_reconciliation_columns() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE spend_executor_claims (
                id TEXT PRIMARY KEY,
                spend_auth_token_id TEXT NOT NULL UNIQUE,
                owner_user_id TEXT NOT NULL,
                executor_execution_id TEXT NOT NULL,
                workload_profile TEXT NOT NULL,
                status TEXT NOT NULL,
                claimed_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                finalized_at TEXT,
                settlement_id TEXT
            );",
        )
        .unwrap();

        let repo = SqliteGovernanceRepository::from_connection(conn)
            .expect("legacy executor claim schema should migrate");

        for column in [
            "provider_reference",
            "reconciliation_evidence",
            "reconciled_at",
            "reconciled_by_user_id",
        ] {
            assert!(table_has_column(&repo.conn, "spend_executor_claims", column).unwrap());
        }
    }

    #[test]
    fn declarative_policy_apply_is_idempotent_versioned_and_compare_and_set() {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        let scope = PolicyAssignmentScope::UserDefault;
        let first = repo
            .apply_policy(
                &user_id(),
                "demo_policy",
                "Demo policy",
                &scope,
                &policy(),
                None,
                None,
                "usr_test",
                "test",
            )
            .unwrap();
        assert!(first.resource.policy_id.starts_with("pol_"));
        assert_eq!(first.resource.policy_id.len(), 16);
        assert_eq!(first.resource.current_revision, 1);
        assert!(first.changed);

        let identical = repo
            .apply_policy(
                &user_id(),
                "demo_policy",
                "Demo policy",
                &scope,
                &policy(),
                Some(1),
                Some(&first.resource.current_hash),
                "usr_test",
                "test",
            )
            .unwrap();
        assert!(!identical.changed);
        assert_eq!(identical.resource.policy_id, first.resource.policy_id);
        assert_eq!(
            repo.load_policy_history(&user_id(), "demo_policy")
                .unwrap()
                .len(),
            1
        );

        let renamed = repo
            .apply_policy(
                &user_id(),
                "demo_policy",
                "Renamed policy",
                &scope,
                &policy(),
                Some(1),
                None,
                "usr_test",
                "test",
            )
            .unwrap();
        assert_eq!(renamed.resource.policy_id, first.resource.policy_id);
        assert_eq!(renamed.resource.current_revision, 1);
        assert_eq!(renamed.resource.display_name, "Renamed policy");

        let mut changed_policy = policy();
        changed_policy.version = "v2".to_string();
        changed_policy.rules[0].reason = "changed reason".to_string();
        let changed = repo
            .apply_policy(
                &user_id(),
                "demo_policy",
                "Renamed policy",
                &scope,
                &changed_policy,
                Some(1),
                None,
                "usr_test",
                "test",
            )
            .unwrap();
        assert_eq!(changed.resource.current_revision, 2);
        assert_eq!(
            repo.load_policy_history(&user_id(), "demo_policy")
                .unwrap()
                .len(),
            2
        );

        let reverted = repo
            .apply_policy(
                &user_id(),
                "demo_policy",
                "Renamed policy",
                &scope,
                &policy(),
                Some(2),
                Some(&changed.resource.current_hash),
                "usr_test",
                "test",
            )
            .unwrap();
        assert_eq!(reverted.resource.current_revision, 3);
        assert_eq!(reverted.resource.current_hash, first.resource.current_hash);
        assert_eq!(
            repo.load_policy_history(&user_id(), "demo_policy")
                .unwrap()
                .len(),
            3
        );

        let stale = repo.apply_policy(
            &user_id(),
            "demo_policy",
            "Stale rename",
            &scope,
            &policy(),
            Some(1),
            None,
            "usr_test",
            "test",
        );
        assert!(stale
            .unwrap_err()
            .to_string()
            .contains("compare-and-set failed"));
        let current = repo
            .load_policy(&user_id(), "demo_policy")
            .unwrap()
            .unwrap();
        assert_eq!(current.current_revision, 3);
        assert_eq!(current.current_hash, first.resource.current_hash);
        assert_eq!(current.display_name, "Renamed policy");
        assert_eq!(
            repo.load_policy_audit(&user_id(), "demo_policy")
                .unwrap()
                .len(),
            4
        );
    }

    #[test]
    fn legacy_scope_assignments_migrate_without_effective_policy_changes() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE policy_assignments (
                owner_user_id TEXT NOT NULL,
                scope_type TEXT NOT NULL,
                scope_id TEXT NOT NULL,
                agent_id TEXT,
                policy_id TEXT NOT NULL,
                policy_version TEXT NOT NULL,
                policy_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY(owner_user_id, scope_type, scope_id)
            );",
        )
        .unwrap();
        let legacy = policy();
        conn.execute(
            "INSERT INTO policy_assignments
             (owner_user_id, scope_type, scope_id, policy_id, policy_version, policy_json,
              created_at, updated_at)
             VALUES (?1, 'user_default', 'default', ?2, ?3, ?4, ?5, ?5)",
            params![
                user_id().to_string(),
                legacy.id,
                legacy.version,
                serde_json::to_string(&legacy).unwrap(),
                Utc::now().to_rfc3339(),
            ],
        )
        .unwrap();
        let mut divergent = legacy.clone();
        divergent.version = "v2-agent-only".to_string();
        divergent.default_effect = Effect::Deny;
        conn.execute(
            "INSERT INTO policy_assignments
             (owner_user_id, scope_type, scope_id, agent_id, policy_id, policy_version,
              policy_json, created_at, updated_at)
             VALUES (?1, 'agent_override', ?2, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![
                user_id().to_string(),
                agent_id().to_string(),
                divergent.id,
                divergent.version,
                serde_json::to_string(&divergent).unwrap(),
                Utc::now().to_rfc3339(),
            ],
        )
        .unwrap();
        let second_agent_id = "00000000-0000-4000-8000-000000000789";
        conn.execute(
            "INSERT INTO policy_assignments
             (owner_user_id, scope_type, scope_id, agent_id, policy_id, policy_version,
              policy_json, created_at, updated_at)
             VALUES (?1, 'agent_override', ?2, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![
                user_id().to_string(),
                second_agent_id,
                divergent.id,
                divergent.version,
                serde_json::to_string(&divergent).unwrap(),
                Utc::now().to_rfc3339(),
            ],
        )
        .unwrap();

        let repo = SqliteGovernanceRepository::from_connection(conn).unwrap();
        let assignments = repo.load_policy_assignments().unwrap();
        assert_eq!(assignments.len(), 3);
        let default = assignments
            .iter()
            .find(|assignment| assignment.scope == PolicyAssignmentScope::UserDefault)
            .unwrap();
        let override_assignments = assignments
            .iter()
            .filter(|assignment| {
                matches!(assignment.scope, PolicyAssignmentScope::AgentOverride(_))
            })
            .collect::<Vec<_>>();
        assert_eq!(override_assignments.len(), 2);
        assert!(default.policy_id.starts_with("pol_"));
        assert_ne!(default.policy_id, override_assignments[0].policy_id);
        assert_eq!(
            override_assignments[0].policy_id,
            override_assignments[1].policy_id
        );
        assert_eq!(default.policy.version, legacy.version);
        assert_eq!(override_assignments[0].policy.version, divergent.version);
        assert_eq!(override_assignments[0].policy.default_effect, Effect::Deny);
        let resource = repo
            .load_policy(&user_id(), "demo_policy")
            .unwrap()
            .unwrap();
        assert_eq!(resource.current_revision, 1);
        let audit = repo.load_policy_audit(&user_id(), "demo_policy").unwrap();
        assert_eq!(audit[0].source, "legacy_assignment_migration");
        assert_eq!(audit[0].affected_assignments, vec!["user_default:default"]);
        let resources = repo.load_policies(&user_id()).unwrap();
        assert_eq!(resources.len(), 2);
        assert!(resources.iter().any(|resource| resource
            .declarative_key
            .starts_with("demo_policy--migrated-")));
    }

    #[test]
    fn executor_settlement_is_atomic_and_idempotent() {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        let settlement_started_at = Utc::now();
        let (claim, _, _) = persist_claimed_executor_spend(
            &mut repo,
            settlement_started_at + Duration::minutes(15),
        );
        let first_settlement_id = PaymentId::new();

        let wrong_operation_error = repo
            .settle_executor_claim_transactionally(
                &claim.owner_user_id,
                &claim.agent_id,
                "another-operation",
                PaymentId::new(),
                settlement_receipt(2_000),
                settlement_started_at,
            )
            .unwrap_err();
        assert!(wrong_operation_error
            .to_string()
            .contains("unknown executor claim"));

        let first = repo
            .settle_executor_claim_transactionally(
                &claim.owner_user_id,
                &claim.agent_id,
                &claim.operation_key,
                first_settlement_id.clone(),
                settlement_receipt(2_000),
                settlement_started_at,
            )
            .unwrap();
        assert!(!first.idempotent_replay);
        assert_eq!(first.claim.settlement_id, Some(first_settlement_id.clone()));
        assert_eq!(
            first.token.used_by_payment_id,
            Some(first_settlement_id.clone())
        );
        assert!(matches!(
            first.claim.status,
            SpendExecutorClaimStatus::Settled
        ));
        assert!(matches!(first.hold.status, BudgetHoldStatus::Settled));
        assert_eq!(first.balance.consumed_amount_cents, 2_000);
        assert_eq!(first.balance.frozen_amount_cents, 0);
        assert_eq!(first.balance.remaining_amount_cents, 8_000);
        let receipt = first.receipt.as_ref().expect("receipt should be returned");
        assert_eq!(receipt.authorized_max_cents, 2_500);
        assert_eq!(receipt.receipt.actual_vendor_cost_cents, 2_000);
        assert_eq!(receipt.released_amount_cents, 500);
        assert_eq!(
            load_executor_settlement_receipt_by_claim_id(&repo.conn, &claim.id)
                .unwrap()
                .expect("receipt should be durable"),
            *receipt
        );

        let replay = repo
            .settle_executor_claim_transactionally(
                &claim.owner_user_id,
                &claim.agent_id,
                &claim.operation_key,
                PaymentId::new(),
                settlement_receipt(2_000),
                settlement_started_at + Duration::seconds(1),
            )
            .unwrap();
        assert!(replay.idempotent_replay);
        assert_eq!(replay.claim.settlement_id, Some(first_settlement_id));
        assert_eq!(replay.balance.consumed_amount_cents, 2_000);
        assert_eq!(replay.balance.frozen_amount_cents, 0);

        let changed_receipt_error = repo
            .settle_executor_claim_transactionally(
                &claim.owner_user_id,
                &claim.agent_id,
                &claim.operation_key,
                PaymentId::new(),
                settlement_receipt(1_999),
                settlement_started_at + Duration::seconds(2),
            )
            .unwrap_err();
        assert!(changed_receipt_error
            .to_string()
            .contains("receipt does not match"));
    }

    #[test]
    fn executor_settlement_rejects_cost_above_authorized_maximum() {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        let settlement_started_at = Utc::now();
        let (claim, token, hold) = persist_claimed_executor_spend(
            &mut repo,
            settlement_started_at + Duration::minutes(15),
        );

        let error = repo
            .settle_executor_claim_transactionally(
                &claim.owner_user_id,
                &claim.agent_id,
                &claim.operation_key,
                PaymentId::new(),
                settlement_receipt(2_501),
                settlement_started_at,
            )
            .unwrap_err();
        assert!(error.to_string().contains("exceeds the authorized maximum"));
        assert!(matches!(
            load_executor_claim_by_id(&repo.conn, &claim.id)
                .unwrap()
                .expect("claim should remain")
                .status,
            SpendExecutorClaimStatus::Claimed
        ));
        assert!(load_spend_auth_token_by_id(&repo.conn, &token.id)
            .unwrap()
            .expect("token should remain")
            .used_at
            .is_none());
        let balance = load_budget_balance_by_id(&repo.conn, &hold.budget_id)
            .unwrap()
            .expect("balance should remain");
        assert_eq!(balance.consumed_amount_cents, 0);
        assert_eq!(balance.frozen_amount_cents, 2_500);
    }

    #[test]
    fn executor_settlement_rolls_back_every_record_on_write_failure() {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        let settlement_started_at = Utc::now();
        let (claim, token, hold) = persist_claimed_executor_spend(
            &mut repo,
            settlement_started_at + Duration::minutes(15),
        );
        repo.conn
            .execute_batch(
                "CREATE TRIGGER fail_executor_receipt_insert
                 BEFORE INSERT ON spend_executor_settlement_receipts
                 BEGIN
                   SELECT RAISE(ABORT, 'injected receipt write failure');
                 END;",
            )
            .unwrap();

        let error = repo
            .settle_executor_claim_transactionally(
                &claim.owner_user_id,
                &claim.agent_id,
                &claim.operation_key,
                PaymentId::new(),
                settlement_receipt(2_000),
                settlement_started_at,
            )
            .unwrap_err();
        assert!(error.to_string().contains("injected receipt write failure"));

        let reloaded_claim = load_executor_claim_by_id(&repo.conn, &claim.id)
            .unwrap()
            .unwrap();
        let reloaded_token = load_spend_auth_token_by_id(&repo.conn, &token.id)
            .unwrap()
            .unwrap();
        let reloaded_hold = load_budget_hold_by_claim_id(&repo.conn, &claim.id)
            .unwrap()
            .unwrap();
        let reloaded_balance = load_budget_balance_by_id(&repo.conn, &hold.budget_id)
            .unwrap()
            .unwrap();
        assert!(matches!(
            reloaded_claim.status,
            SpendExecutorClaimStatus::Claimed
        ));
        assert_eq!(reloaded_claim.settlement_id, None);
        assert_eq!(reloaded_token.used_at, None);
        assert_eq!(reloaded_token.used_by_payment_id, None);
        assert!(matches!(reloaded_hold.status, BudgetHoldStatus::Claimed));
        assert_eq!(reloaded_balance.consumed_amount_cents, 0);
        assert_eq!(reloaded_balance.frozen_amount_cents, 2_500);
        assert!(
            load_executor_settlement_receipt_by_claim_id(&repo.conn, &claim.id)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn executor_settlement_uses_one_start_time_for_claim_expiry() {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        let settlement_started_at = Utc::now();
        let (claim, token, _) = persist_claimed_executor_spend(&mut repo, settlement_started_at);

        let error = repo
            .settle_executor_claim_transactionally(
                &claim.owner_user_id,
                &claim.agent_id,
                &claim.operation_key,
                PaymentId::new(),
                settlement_receipt(2_000),
                settlement_started_at,
            )
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("expired and requires reconciliation"));
        assert_eq!(
            load_spend_auth_token_by_id(&repo.conn, &token.id)
                .unwrap()
                .unwrap()
                .used_at,
            None
        );
    }

    #[test]
    fn expired_executor_claim_can_be_reconciled_as_billed_with_durable_evidence() {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        let reconciliation_started_at = Utc::now();
        let (claim, _, _) = persist_claimed_executor_spend(&mut repo, reconciliation_started_at);
        let settlement_id = PaymentId::new();

        let reconciled = repo
            .reconcile_executor_claim_as_billed_transactionally(
                &claim.id,
                &claim.owner_user_id,
                " vendor-charge-123 ",
                " Invoice confirms the vendor charge. ",
                settlement_id.clone(),
                settlement_receipt(2_000),
                reconciliation_started_at,
            )
            .unwrap();

        assert!(!reconciled.idempotent_replay);
        assert!(matches!(
            reconciled.claim.status,
            SpendExecutorClaimStatus::Settled
        ));
        assert_eq!(reconciled.claim.settlement_id, Some(settlement_id.clone()));
        assert_eq!(
            reconciled.claim.provider_reference.as_deref(),
            Some("vendor-charge-123")
        );
        assert_eq!(
            reconciled.claim.reconciliation_evidence.as_deref(),
            Some("Invoice confirms the vendor charge.")
        );
        assert_eq!(
            reconciled.claim.reconciled_at,
            Some(reconciliation_started_at)
        );
        assert_eq!(
            reconciled.claim.reconciled_by_user_id,
            Some(claim.owner_user_id.clone())
        );
        assert_eq!(reconciled.balance.consumed_amount_cents, 2_000);
        assert_eq!(reconciled.balance.remaining_amount_cents, 8_000);
        assert_eq!(reconciled.balance.frozen_amount_cents, 0);
        let reloaded = load_executor_claim_by_id(&repo.conn, &claim.id)
            .unwrap()
            .expect("reconciled claim should persist");
        assert_eq!(
            reloaded.provider_reference,
            reconciled.claim.provider_reference
        );
        assert_eq!(
            reloaded.reconciliation_evidence,
            reconciled.claim.reconciliation_evidence
        );
        assert_eq!(reloaded.reconciled_at, reconciled.claim.reconciled_at);
        assert_eq!(
            reloaded.reconciled_by_user_id,
            reconciled.claim.reconciled_by_user_id
        );

        let replay = repo
            .reconcile_executor_claim_as_billed_transactionally(
                &claim.id,
                &claim.owner_user_id,
                "vendor-charge-123",
                "Invoice confirms the vendor charge.",
                PaymentId::new(),
                settlement_receipt(2_000),
                reconciliation_started_at + Duration::seconds(1),
            )
            .unwrap();
        assert!(replay.idempotent_replay);
        assert_eq!(replay.claim.settlement_id, Some(settlement_id));

        let changed_evidence_error = repo
            .reconcile_executor_claim_as_billed_transactionally(
                &claim.id,
                &claim.owner_user_id,
                "vendor-charge-123",
                "Different evidence",
                PaymentId::new(),
                settlement_receipt(2_000),
                reconciliation_started_at + Duration::seconds(2),
            )
            .unwrap_err();
        assert!(changed_evidence_error
            .to_string()
            .contains("different evidence"));
    }

    #[test]
    fn expired_executor_claim_can_be_reconciled_as_not_billed() {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        let reconciliation_started_at = Utc::now();
        let (claim, _, _) = persist_claimed_executor_spend(&mut repo, reconciliation_started_at);

        let reconciled = repo
            .reconcile_executor_claim_as_not_billed_transactionally(
                &claim.id,
                &claim.owner_user_id,
                "vendor-job-456",
                "Provider billing search found no charge.",
                reconciliation_started_at,
            )
            .unwrap();

        assert!(matches!(
            reconciled.claim.status,
            SpendExecutorClaimStatus::Released
        ));
        assert!(reconciled.token.revoked_at.is_some());
        assert_eq!(reconciled.balance.frozen_amount_cents, 0);
        assert_eq!(reconciled.balance.remaining_amount_cents, 10_000);
    }

    #[test]
    fn active_executor_claim_cannot_use_reconciliation_path() {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        let reconciliation_started_at = Utc::now();
        let (claim, _, _) = persist_claimed_executor_spend(
            &mut repo,
            reconciliation_started_at + Duration::minutes(15),
        );

        let error = repo
            .reconcile_executor_claim_as_not_billed_transactionally(
                &claim.id,
                &claim.owner_user_id,
                "vendor-job-active",
                "No charge yet.",
                reconciliation_started_at,
            )
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("does not require reconciliation"));
    }

    #[test]
    fn executor_release_is_atomic_idempotent_and_blocks_settlement() {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        let finalization_started_at = Utc::now();
        let (claim, _, _) = persist_claimed_executor_spend(
            &mut repo,
            finalization_started_at + Duration::minutes(15),
        );

        let released = repo
            .release_executor_claim_transactionally(
                &claim.owner_user_id,
                &claim.agent_id,
                &claim.operation_key,
                finalization_started_at,
            )
            .unwrap();
        assert!(!released.idempotent_replay);
        assert!(matches!(
            released.claim.status,
            SpendExecutorClaimStatus::Released
        ));
        assert!(released.token.revoked_at.is_some());
        assert!(matches!(released.hold.status, BudgetHoldStatus::Released));
        assert_eq!(released.balance.frozen_amount_cents, 0);
        assert_eq!(released.balance.remaining_amount_cents, 10_000);

        let replay = repo
            .release_executor_claim_transactionally(
                &claim.owner_user_id,
                &claim.agent_id,
                &claim.operation_key,
                finalization_started_at + Duration::seconds(1),
            )
            .unwrap();
        assert!(replay.idempotent_replay);
        assert_eq!(replay.balance.remaining_amount_cents, 10_000);

        let settle_error = repo
            .settle_executor_claim_transactionally(
                &claim.owner_user_id,
                &claim.agent_id,
                &claim.operation_key,
                PaymentId::new(),
                settlement_receipt(2_000),
                finalization_started_at + Duration::seconds(1),
            )
            .unwrap_err();
        assert!(settle_error.to_string().contains("already been released"));
    }

    #[test]
    fn persists_policy_assignment_and_spend_records() {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        repo.save_policy_assignment(&user_id(), &PolicyAssignmentScope::UserDefault, &policy())
            .unwrap();

        let decision = spend_decision();
        let token = SpendAuthTokenRecord {
            id: SpendAuthTokenId::new(),
            owner_user_id: user_id(),
            spend_decision_id: decision.id.clone(),
            expires_at: Utc::now() + Duration::minutes(5),
            claim_ttl_seconds: 900,
            used_at: None,
            used_by_payment_id: None,
            revoked_at: None,
        };

        repo.save_spend_decision(&decision).unwrap();
        repo.save_spend_auth_token(&token).unwrap();
        let claim = SpendExecutorClaimRecord {
            id: SpendExecutorClaimId::new(),
            spend_auth_token_id: token.id.clone(),
            owner_user_id: user_id(),
            agent_id: decision.request.agent_id.clone(),
            operation_key: decision.operation_key.clone(),
            workload_profile: "default".to_string(),
            status: SpendExecutorClaimStatus::Claimed,
            claimed_at: Utc::now(),
            expires_at: Utc::now() + Duration::minutes(15),
            finalized_at: None,
            settlement_id: None,
            provider_reference: None,
            reconciliation_evidence: None,
            reconciled_at: None,
            reconciled_by_user_id: None,
        };
        repo.save_executor_claim(&claim).unwrap();

        let assignments = repo.load_policy_assignments().unwrap();
        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].scope, PolicyAssignmentScope::UserDefault);
        assert_eq!(repo.load_spend_decisions().unwrap().len(), 1);
        assert_eq!(repo.load_spend_auth_tokens().unwrap().len(), 1);
        let claims = repo.load_executor_claims().unwrap();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].id, claim.id);
        assert_eq!(claims[0].operation_key, decision.operation_key);
    }

    #[test]
    fn spend_operation_key_uniqueness_is_scoped_to_the_agent() {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        let first = spend_decision();
        repo.save_spend_decision(&first).unwrap();

        let mut second = spend_decision();
        second.operation_key = first.operation_key.clone();
        second.request.agent_id = AgentId::new();
        repo.save_spend_decision(&second)
            .expect("another agent should be able to reuse the operation key");

        let mut duplicate = spend_decision();
        duplicate.operation_key = first.operation_key.clone();
        duplicate.request.agent_id = first.request.agent_id.clone();
        let error = repo
            .save_spend_decision(&duplicate)
            .expect_err("one agent must not reuse an operation key");
        assert!(error.to_string().contains("UNIQUE constraint failed"));
    }

    #[test]
    fn corrected_attempt_admission_is_atomic_across_separate_sqlite_connections() {
        let path = std::env::temp_dir().join(format!(
            "hubu-corrected-attempt-concurrency-{}.sqlite",
            UserId::new()
        ));
        let mut seed = SqliteGovernanceRepository::open(&path).unwrap();
        let initial_request = spend_request();
        let initial = seed
            .admit_spend_attempt(
                &user_id(),
                "corrected-concurrent",
                &initial_request,
                "test:initial",
                Utc::now(),
            )
            .unwrap();
        assert_eq!(initial, SpendAttemptAdmission::Admitted { revision: 1 });
        let mut denied = denied_spend_decision(1, initial_request.clone());
        denied.operation_key = "corrected-concurrent".to_string();
        seed.save_spend_decision(&denied).unwrap();
        seed.record_spend_attempt_outcome(
            &denied,
            SpendAuthorizationDecision::Denied,
            &denied.evaluation.reasons,
            Utc::now(),
        )
        .unwrap();
        drop(seed);

        let first_repo = SqliteGovernanceRepository::open(&path).unwrap();
        let second_repo = SqliteGovernanceRepository::open(&path).unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let handles = [first_repo, second_repo]
            .into_iter()
            .enumerate()
            .map(|(index, mut repo)| {
                let barrier = Arc::clone(&barrier);
                let mut corrected = initial_request.clone();
                corrected.amount_cents += index as i64 + 1;
                std::thread::spawn(move || {
                    barrier.wait();
                    repo.admit_spend_attempt(
                        &user_id(),
                        "corrected-concurrent",
                        &corrected,
                        &format!("test:concurrent-{index}"),
                        Utc::now(),
                    )
                    .unwrap()
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, SpendAttemptAdmission::Admitted { .. }))
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, SpendAttemptAdmission::ChangedScopeBlocked))
                .count(),
            1
        );

        let audit = SqliteGovernanceRepository::open(&path)
            .unwrap()
            .load_spend_attempt_history(&agent_id(), "corrected-concurrent")
            .unwrap();
        assert_eq!(audit.len(), 2);
        assert_eq!(audit[0].final_decision, SpendAuthorizationDecision::Denied);
        assert_eq!(
            audit[1].final_decision,
            SpendAuthorizationDecision::PendingApproval
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn legacy_allow_without_token_migrates_pending_and_blocks_changed_scope() {
        let path =
            std::env::temp_dir().join(format!("hubu-legacy-unsafe-allow-{}.sqlite", UserId::new()));
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE spend_decisions (
                id TEXT PRIMARY KEY,
                owner_user_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                operation_key TEXT NOT NULL,
                request_json TEXT NOT NULL,
                evaluation_json TEXT NOT NULL,
                created_at TEXT NOT NULL
             );",
        )
        .unwrap();
        let legacy = spend_decision();
        conn.execute(
            "INSERT INTO spend_decisions
             (id, owner_user_id, agent_id, operation_key, request_json, evaluation_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                legacy.id.to_string(),
                legacy.owner_user_id.to_string(),
                legacy.request.agent_id.to_string(),
                legacy.operation_key,
                serde_json::to_string(&legacy.request).unwrap(),
                serde_json::to_string(&legacy.evaluation).unwrap(),
                legacy.created_at.to_rfc3339(),
            ],
        )
        .unwrap();
        drop(conn);

        let mut repo = SqliteGovernanceRepository::open(&path).unwrap();
        let audit = repo
            .load_spend_attempt_history(&legacy.request.agent_id, &legacy.operation_key)
            .unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(
            audit[0].final_decision,
            SpendAuthorizationDecision::PendingApproval
        );
        assert_eq!(
            repo.admit_spend_attempt(
                &legacy.owner_user_id,
                &legacy.operation_key,
                &legacy.request,
                "test:replay",
                Utc::now(),
            )
            .unwrap(),
            SpendAttemptAdmission::ExactReplay { revision: 1 }
        );
        let mut changed = legacy.request.clone();
        changed.amount_cents += 1;
        assert_eq!(
            repo.admit_spend_attempt(
                &legacy.owner_user_id,
                &legacy.operation_key,
                &changed,
                "test:changed",
                Utc::now(),
            )
            .unwrap(),
            SpendAttemptAdmission::ChangedScopeBlocked
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn pre_scope_attempt_exact_replay_recovers_token_and_blocks_changed_scope() {
        let path = std::env::temp_dir().join(format!(
            "hubu-pre-scope-exact-replay-{}.sqlite",
            UserId::new()
        ));
        let mut repo = SqliteGovernanceRepository::open(&path).unwrap();
        let mut decision = spend_decision();
        decision.operation_key = "pre-scope-token-recovery".into();
        decision.request.merchant = Some("Acme".into());
        decision.request.execution_scope = None;
        let mut legacy_request = serde_json::to_value(&decision.request).unwrap();
        legacy_request
            .as_object_mut()
            .unwrap()
            .remove("execution_scope");
        let legacy_request_json = serde_json::to_string(&legacy_request).unwrap();
        repo.conn
            .execute(
                "INSERT INTO spend_operation_attempts
                 (agent_id, operation_key, revision, owner_user_id, request_json, actor, submitted_at)
                 VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6)",
                params![
                    decision.request.agent_id.to_string(),
                    decision.operation_key,
                    decision.owner_user_id.to_string(),
                    legacy_request_json,
                    decision.actor,
                    decision.created_at.to_rfc3339(),
                ],
            )
            .unwrap();
        repo.conn
            .execute(
                "INSERT INTO spend_decisions
                 (id, owner_user_id, agent_id, operation_key, revision, actor,
                  request_json, evaluation_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7, ?8)",
                params![
                    decision.id.to_string(),
                    decision.owner_user_id.to_string(),
                    decision.request.agent_id.to_string(),
                    decision.operation_key,
                    decision.actor,
                    legacy_request_json,
                    serde_json::to_string(&decision.evaluation).unwrap(),
                    decision.created_at.to_rfc3339(),
                ],
            )
            .unwrap();
        let token = SpendAuthTokenRecord {
            id: SpendAuthTokenId::new(),
            owner_user_id: decision.owner_user_id.clone(),
            spend_decision_id: decision.id.clone(),
            expires_at: Utc::now() + Duration::minutes(5),
            claim_ttl_seconds: 900,
            used_at: None,
            used_by_payment_id: None,
            revoked_at: None,
        };
        repo.save_spend_auth_token(&token).unwrap();
        drop(repo);

        let mut repo = SqliteGovernanceRepository::open(&path).unwrap();
        let mut upgraded = decision.request.clone();
        upgraded.execution_scope = Some(legacy_execution_scope("Acme"));
        assert_eq!(
            repo.admit_spend_attempt(
                &decision.owner_user_id,
                &decision.operation_key,
                &upgraded,
                "test:retry",
                Utc::now(),
            )
            .unwrap(),
            SpendAttemptAdmission::ExactReplay { revision: 1 }
        );

        let mut manager = crate::spend::SpendManager::from_records(
            repo.load_spend_decisions().unwrap(),
            repo.load_spend_auth_tokens().unwrap(),
        );
        let replay = manager
            .evaluate_spend(
                &hubu_common::models::UserContext::new(decision.owner_user_id.clone()),
                &decision.operation_key,
                upgraded.clone(),
                &policy(),
            )
            .unwrap();
        assert!(replay.idempotent_replay);
        assert_eq!(replay.auth_token.unwrap().id, token.id);

        let mut changed = upgraded;
        changed.execution_scope = Some(
            serde_json::from_str(include_str!("../../../fixtures/execution-scope-v1.json"))
                .unwrap(),
        );
        assert_eq!(
            repo.admit_spend_attempt(
                &decision.owner_user_id,
                &decision.operation_key,
                &changed,
                "test:changed",
                Utc::now(),
            )
            .unwrap(),
            SpendAttemptAdmission::ChangedScopeBlocked
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn migrates_duplicate_legacy_execution_ids_to_unique_operation_keys() {
        let path = std::env::temp_dir().join(format!(
            "hubu-governance-operation-migration-{}.sqlite",
            UserId::new()
        ));
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE spend_decisions (
                id TEXT PRIMARY KEY,
                owner_user_id TEXT NOT NULL,
                request_json TEXT NOT NULL,
                evaluation_json TEXT NOT NULL,
                created_at TEXT NOT NULL
             );
             CREATE TABLE spend_auth_tokens (
                id TEXT PRIMARY KEY,
                owner_user_id TEXT NOT NULL,
                spend_decision_id TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                claim_ttl_seconds INTEGER NOT NULL DEFAULT 900,
                used_at TEXT,
                used_by_payment_id TEXT,
                revoked_at TEXT
             );
             CREATE TABLE spend_executor_claims (
                id TEXT PRIMARY KEY,
                spend_auth_token_id TEXT NOT NULL UNIQUE,
                owner_user_id TEXT NOT NULL,
                executor_execution_id TEXT NOT NULL,
                workload_profile TEXT NOT NULL,
                status TEXT NOT NULL,
                claimed_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                finalized_at TEXT,
                settlement_id TEXT
             );",
        )
        .unwrap();

        for _ in 0..2 {
            let decision = spend_decision();
            let token_id = SpendAuthTokenId::new();
            let claim_id = SpendExecutorClaimId::new();
            conn.execute(
                "INSERT INTO spend_decisions
                 (id, owner_user_id, request_json, evaluation_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    decision.id.to_string(),
                    decision.owner_user_id.to_string(),
                    serde_json::to_string(&decision.request).unwrap(),
                    serde_json::to_string(&decision.evaluation).unwrap(),
                    decision.created_at.to_rfc3339(),
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO spend_auth_tokens
                 (id, owner_user_id, spend_decision_id, expires_at, claim_ttl_seconds)
                 VALUES (?1, ?2, ?3, ?4, 900)",
                params![
                    token_id.to_string(),
                    user_id().to_string(),
                    decision.id.to_string(),
                    (Utc::now() + Duration::minutes(5)).to_rfc3339(),
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO spend_executor_claims
                 (id, spend_auth_token_id, owner_user_id, executor_execution_id,
                  workload_profile, status, claimed_at, expires_at)
                 VALUES (?1, ?2, ?3, 'reused-legacy-execution', 'default',
                         'claimed', ?4, ?5)",
                params![
                    claim_id.to_string(),
                    token_id.to_string(),
                    user_id().to_string(),
                    Utc::now().to_rfc3339(),
                    (Utc::now() + Duration::minutes(15)).to_rfc3339(),
                ],
            )
            .unwrap();
        }
        drop(conn);

        let repo = SqliteGovernanceRepository::open(&path).expect("legacy database should migrate");
        let claims = repo.load_executor_claims().unwrap();
        let decisions = repo.load_spend_decisions().unwrap();
        let tokens = repo.load_spend_auth_tokens().unwrap();
        assert!(table_has_column(&repo.conn, "spend_decisions", "operation_key").unwrap());
        assert!(table_has_column(&repo.conn, "spend_executor_claims", "operation_key").unwrap());
        assert!(!table_has_column(&repo.conn, "spend_decisions", "job_id").unwrap());
        assert!(!table_has_column(&repo.conn, "spend_executor_claims", "job_id").unwrap());
        assert!(
            !table_has_column(&repo.conn, "spend_executor_claims", "executor_execution_id")
                .unwrap()
        );
        assert_eq!(claims.len(), 2);
        assert_ne!(claims[0].operation_key, claims[1].operation_key);
        for claim in claims {
            let token = tokens
                .iter()
                .find(|token| token.id == claim.spend_auth_token_id)
                .expect("claim token should migrate");
            let decision = decisions
                .iter()
                .find(|decision| decision.id == token.spend_decision_id)
                .expect("claim decision should migrate");
            assert_eq!(claim.operation_key, decision.operation_key);
        }
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn expire_overdue_budget_holds_returns_frozen_amount_to_remaining() {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        let decision = spend_decision();
        repo.save_spend_decision(&decision).unwrap();

        let budget = Budget::new(
            BudgetId::new(),
            AgentId::new(),
            10_000,
            Currency::Usd,
            TimePeriod::new(
                Utc::now() - Duration::hours(1),
                Some(Utc::now() + Duration::hours(1)),
            )
            .unwrap(),
        )
        .unwrap();
        let reserved_balance = BudgetBalance {
            budget_id: budget.id.clone(),
            consumed_amount_cents: 0,
            frozen_amount_cents: 2_500,
            remaining_amount_cents: 7_500,
        };
        let hold = BudgetHold {
            id: BudgetHoldId::new(),
            budget_id: budget.id.clone(),
            spend_decision_id: decision.id,
            amount_cents: 2_500,
            currency: Currency::Usd,
            status: BudgetHoldStatus::Frozen,
            executor_claim_id: None,
            created_at: Utc::now() - Duration::minutes(10),
            updated_at: Utc::now() - Duration::minutes(10),
            expires_at: Utc::now() - Duration::minutes(5),
        };

        repo.save_budget_with_balance(&budget, &reserved_balance)
            .unwrap();
        repo.save_budget_hold(&hold, &reserved_balance).unwrap();
        repo.expire_overdue_budget_holds(Utc::now()).unwrap();

        let reloaded_hold = repo.load_budget_holds().unwrap().pop().unwrap();
        let reloaded_balance = repo.load_budget_balances().unwrap().pop().unwrap();
        assert!(matches!(reloaded_hold.status, BudgetHoldStatus::Expired));
        assert_eq!(reloaded_balance.frozen_amount_cents, 0);
        assert_eq!(reloaded_balance.remaining_amount_cents, 10_000);
    }

    #[test]
    fn persists_spending_targets_separately_from_budgets() {
        let mut repo = SqliteGovernanceRepository::in_memory().unwrap();
        let now = Utc::now();
        let target = SpendingTarget {
            id: SpendingTargetId::new(),
            owner_user_id: user_id(),
            target_amount_cents: 25_000,
            currency: Currency::Usd,
            period: TimePeriod::new(now, Some(now + Duration::days(30))).unwrap(),
            status: SpendingTargetStatus::Active,
            created_at: now,
            updated_at: now,
        };

        repo.save_spending_target(&target).unwrap();

        let targets = repo.load_spending_targets().unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].id, target.id);
        assert_eq!(targets[0].target_amount_cents, 25_000);
        assert!(repo.load_budgets().unwrap().is_empty());
    }

    #[test]
    fn migrates_legacy_user_cap_budget_to_advisory_spending_target() {
        let path = std::env::temp_dir().join(format!(
            "hubu-governance-target-migration-{}.sqlite",
            UserId::new()
        ));
        SqliteGovernanceRepository::open(&path).unwrap();
        let legacy_cap_id = BudgetId::new();
        let agent_budget_id = BudgetId::new();
        let spend_decision_id = SpendDecisionId::new();
        let cap_hold_id = BudgetHoldId::new();
        let agent_hold_id = BudgetHoldId::new();
        let now = Utc::now();
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute("DROP INDEX budget_holds_one_per_spend_decision", [])
                .unwrap();
            conn.execute(
                "INSERT INTO spend_decisions
                 (id, owner_user_id, agent_id, operation_key, request_json, evaluation_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, '{}', '{}', ?5)",
                params![
                    spend_decision_id.to_string(),
                    user_id().to_string(),
                    agent_id().to_string(),
                    format!("legacy-{spend_decision_id}"),
                    now.to_rfc3339(),
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO budgets
                 (id, scope_type, scope_id, amount_limit_cents, currency, starting_at,
                  ending_before, status, created_at, updated_at)
                 VALUES (?1, 'user', ?2, 10000, 'usd', ?3, NULL, 'active', ?3, ?3)",
                params![
                    legacy_cap_id.to_string(),
                    user_id().to_string(),
                    now.to_rfc3339(),
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO budgets
                 (id, scope_type, scope_id, amount_limit_cents, currency, starting_at,
                  ending_before, status, created_at, updated_at)
                 VALUES (?1, 'agent', ?2, 10000, 'usd', ?3, NULL, 'active', ?3, ?3)",
                params![
                    agent_budget_id.to_string(),
                    agent_id().to_string(),
                    now.to_rfc3339(),
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO budget_balances
                 (budget_id, consumed_amount_cents, frozen_amount_cents,
                  remaining_amount_cents, updated_at)
                 VALUES (?1, 0, 500, 9500, ?2)",
                params![legacy_cap_id.to_string(), now.to_rfc3339()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO budget_balances
                 (budget_id, consumed_amount_cents, frozen_amount_cents,
                  remaining_amount_cents, updated_at)
                 VALUES (?1, 0, 500, 9500, ?2)",
                params![agent_budget_id.to_string(), now.to_rfc3339()],
            )
            .unwrap();
            for (hold_id, budget_id) in [
                (&cap_hold_id, &legacy_cap_id),
                (&agent_hold_id, &agent_budget_id),
            ] {
                conn.execute(
                    "INSERT INTO budget_holds
                     (id, budget_id, spend_decision_id, amount_cents, currency,
                      status, created_at, updated_at, expires_at)
                     VALUES (?1, ?2, ?3, 500, 'usd', 'frozen', ?4, ?4, ?5)",
                    params![
                        hold_id.to_string(),
                        budget_id.to_string(),
                        spend_decision_id.to_string(),
                        now.to_rfc3339(),
                        (now + Duration::hours(1)).to_rfc3339(),
                    ],
                )
                .unwrap();
            }
        }

        let repo = SqliteGovernanceRepository::open(&path).unwrap();
        let targets = repo.load_spending_targets().unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].id.to_string(), legacy_cap_id.to_string());
        assert_eq!(targets[0].target_amount_cents, 10_000);
        assert_eq!(targets[0].status, SpendingTargetStatus::Active);
        let budgets = repo.load_budgets().unwrap();
        assert_eq!(budgets.len(), 1);
        assert_eq!(budgets[0].id, agent_budget_id);
        let balances = repo.load_budget_balances().unwrap();
        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].budget_id, agent_budget_id);
        let holds = repo.load_budget_holds().unwrap();
        assert_eq!(holds.len(), 1);
        assert_eq!(holds[0].id, agent_hold_id);
        assert_eq!(holds[0].budget_id, agent_budget_id);
        std::fs::remove_file(path).ok();
    }
}
