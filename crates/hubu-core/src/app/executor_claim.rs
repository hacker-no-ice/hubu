use chrono::{DateTime, Utc};
use hubu_common::ids::{AgentId, PaymentId, SpendExecutorClaimId, UserId};
use serde_json::json;

use crate::{
    budget::{
        BudgetBalance, BudgetHold, BudgetHoldStatus, BudgetManager, BudgetManagerError,
        ReserveBudgetResponse,
    },
    persistence::{ExecutorClaimRepository, ExecutorFinalizationResult},
    spend::{
        PersistedSpendExecutorSettlementReceipt, SpendAuthTokenRecord, SpendDecisionRecord,
        SpendExecutorClaimRecord, SpendExecutorClaimRequest, SpendExecutorClaimStatus,
        SpendExecutorSettlementReceipt, SpendManager, SpendPaymentValidationRequest,
        ValidatedSpendAuthorization,
    },
    storage::StorageError,
    telemetry::log_event,
};

/// Application service for the complete external-executor claim lifecycle.
///
/// HTTP authentication, public-id parsing, and response rendering stay at the
/// transport boundary. This service owns claim creation, lookup, reconciliation
/// selection, and terminal state orchestration across spend, budget, and
/// durable persistence.
pub struct ExecutorClaimService;

#[derive(Debug, Clone)]
pub struct ClaimExecutorSpendRequest {
    pub authorization: SpendPaymentValidationRequest,
    pub operation_key: String,
}

#[derive(Debug, Clone)]
pub struct FinalizeExecutorClaimRequest {
    pub owner_user_id: UserId,
    pub agent_id: AgentId,
    pub operation_key: String,
}

#[derive(Debug, Clone)]
pub struct SettleExecutorClaimRequest {
    pub owner_user_id: UserId,
    pub agent_id: AgentId,
    pub operation_key: String,
    pub receipt: SpendExecutorSettlementReceipt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutorClaimReconciliationOutcome {
    VendorBilled,
    VendorDidNotBill,
}

impl ExecutorClaimReconciliationOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VendorBilled => "vendor_billed",
            Self::VendorDidNotBill => "vendor_did_not_bill",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReconcileExecutorClaimRequest {
    pub claim_id: SpendExecutorClaimId,
    pub owner_user_id: UserId,
    pub provider_reference: String,
    pub evidence: String,
    pub outcome: ExecutorClaimReconciliationOutcome,
    pub receipt: Option<SpendExecutorSettlementReceipt>,
}

#[derive(Debug, Clone)]
pub struct ExecutorClaimState {
    pub claim: SpendExecutorClaimRecord,
    pub decision: SpendDecisionRecord,
    pub token: SpendAuthTokenRecord,
    pub authorization: ValidatedSpendAuthorization,
    pub budget_hold: BudgetHold,
    pub budget_balance: BudgetBalance,
    pub settlement_receipt: Option<PersistedSpendExecutorSettlementReceipt>,
    pub idempotent_replay: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutorClaimServiceError {
    #[error("executor spend operation_key is required")]
    EmptyOperationKey,

    #[error("spend authorization does not have a budget hold")]
    MissingBudgetHold,

    #[error("spend authorization budget hold is not frozen")]
    BudgetHoldNotFrozen,

    #[error("spend authorization budget hold does not match the existing claim")]
    ExistingClaimBudgetHoldMismatch,

    #[error("spend authorization hold does not belong to the authorized agent")]
    BudgetHoldAgentMismatch,

    #[error("spend authorization budget balance is missing")]
    MissingBudgetBalance,

    #[error("executor claim resolved a different spend decision")]
    ClaimDecisionMismatch,

    #[error("unknown executor spend claim")]
    UnknownExecutorClaim,

    #[error("executor claim token is missing")]
    MissingClaimToken,

    #[error("executor claim spend decision is missing")]
    MissingClaimDecision,

    #[error("executor claim owner does not match spend decision")]
    ClaimOwnerMismatch,

    #[error("executor claim budget hold is missing")]
    MissingClaimBudgetHold,

    #[error("executor claim budget hold does not match persisted spend state")]
    ClaimBudgetHoldMismatch,

    #[error("executor claim budget balance is missing")]
    MissingClaimBudgetBalance,

    #[error("settled executor claim is missing settlement id")]
    MissingSettlementId,

    #[error("billed executor settlement is missing a provider receipt")]
    MissingSettlementReceipt,

    #[error("unbilled executor release cannot include a provider receipt")]
    UnexpectedSettlementReceipt,

    #[error("provider reference cannot be empty")]
    EmptyProviderReference,

    #[error("reconciliation evidence cannot be empty")]
    EmptyReconciliationEvidence,

    #[error(transparent)]
    Spend(#[from] crate::spend::SpendError),

    #[error(transparent)]
    Budget(#[from] BudgetManagerError),

    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl ExecutorClaimService {
    pub fn claim<R>(
        &self,
        request: ClaimExecutorSpendRequest,
        spend_manager: &mut SpendManager,
        budget_manager: &mut BudgetManager,
        repository: &mut R,
    ) -> Result<ExecutorClaimState, ExecutorClaimServiceError>
    where
        R: ExecutorClaimRepository,
    {
        let operation_key = request.operation_key.trim();
        if operation_key.is_empty() {
            return Err(ExecutorClaimServiceError::EmptyOperationKey);
        }

        let (prevalidated, existing_claim) = spend_manager
            .validate_auth_token_for_executor_claim(&request.authorization, operation_key)?;
        let budget_hold = budget_manager
            .get_budget_hold_by_spend_decision(&prevalidated.spend_decision_id)
            .ok_or(ExecutorClaimServiceError::MissingBudgetHold)?;
        match existing_claim.as_ref() {
            None if matches!(budget_hold.status, BudgetHoldStatus::Frozen) => {}
            Some(existing) if budget_hold.executor_claim_id.as_ref() == Some(&existing.id) => {}
            None => return Err(ExecutorClaimServiceError::BudgetHoldNotFrozen),
            Some(_) => {
                return Err(ExecutorClaimServiceError::ExistingClaimBudgetHoldMismatch);
            }
        }
        if !budget_manager
            .get_budget_by_id(&budget_hold.budget_id)
            .is_some_and(|budget| budget.budget.agent_id == request.authorization.agent_id)
        {
            return Err(ExecutorClaimServiceError::BudgetHoldAgentMismatch);
        }

        let idempotent_replay = existing_claim.is_some();
        let (claim, authorization) = spend_manager.claim_auth_token(SpendExecutorClaimRequest {
            authorization: request.authorization,
            operation_key: operation_key.to_string(),
        })?;
        if authorization.spend_decision_id != prevalidated.spend_decision_id {
            return Err(ExecutorClaimServiceError::ClaimDecisionMismatch);
        }

        let claimed_hold = if idempotent_replay {
            let balance = budget_manager
                .get_budget_balance(&budget_hold.budget_id)
                .ok_or(ExecutorClaimServiceError::MissingBudgetBalance)?;
            ReserveBudgetResponse {
                hold: budget_hold,
                balance,
            }
        } else {
            budget_manager.claim_budget(&budget_hold.id, claim.id.clone(), claim.expires_at)?
        };
        repository.save_executor_claim_with_budget_hold(
            &claim,
            &claimed_hold.hold,
            &claimed_hold.balance,
        )?;

        let state = claim_state_from_records(
            claim,
            authorization,
            claimed_hold.hold,
            claimed_hold.balance,
            None,
            idempotent_replay,
            spend_manager,
        )?;
        log_event(
            "info",
            "executor_spend_claimed",
            json!({
                "claim_id": state.claim.id.to_string(),
                "operation_key": state.claim.operation_key,
                "spend_auth_token_id": state.claim.spend_auth_token_id.to_string(),
                "claim_expires_at": state.claim.expires_at.to_rfc3339(),
                "workload_profile": state.claim.workload_profile,
                "idempotent_replay": state.idempotent_replay,
            }),
        );
        Ok(state)
    }

    pub fn get(
        &self,
        claim_id: &SpendExecutorClaimId,
        owner_user_id: &UserId,
        spend_manager: &SpendManager,
        budget_manager: &BudgetManager,
    ) -> Result<ExecutorClaimState, ExecutorClaimServiceError> {
        let claim = spend_manager
            .executor_claim_record(claim_id)
            .filter(|claim| &claim.owner_user_id == owner_user_id)
            .ok_or(ExecutorClaimServiceError::UnknownExecutorClaim)?;
        claim_state(claim, false, spend_manager, budget_manager)
    }

    pub fn list_requiring_reconciliation(
        &self,
        owner_user_id: &UserId,
        now: DateTime<Utc>,
        spend_manager: &SpendManager,
        budget_manager: &BudgetManager,
    ) -> Result<Vec<ExecutorClaimState>, ExecutorClaimServiceError> {
        spend_manager
            .executor_claim_records_for_owner(owner_user_id)
            .into_iter()
            .filter(|claim| {
                matches!(claim.status, SpendExecutorClaimStatus::Claimed) && claim.expires_at <= now
            })
            .map(|claim| claim_state(claim, false, spend_manager, budget_manager))
            .collect()
    }

    pub fn settle<R>(
        &self,
        request: SettleExecutorClaimRequest,
        started_at: DateTime<Utc>,
        spend_manager: &mut SpendManager,
        budget_manager: &mut BudgetManager,
        repository: &mut R,
    ) -> Result<ExecutorClaimState, ExecutorClaimServiceError>
    where
        R: ExecutorClaimRepository,
    {
        let finalization = repository.settle_executor_claim_transactionally(
            &request.owner_user_id,
            &request.agent_id,
            &request.operation_key,
            PaymentId::new(),
            request.receipt,
            started_at,
        )?;
        let state = apply_finalization(finalization, spend_manager, budget_manager)?;
        let settlement_id = state
            .claim
            .settlement_id
            .as_ref()
            .ok_or(ExecutorClaimServiceError::MissingSettlementId)?;
        let receipt = state
            .settlement_receipt
            .as_ref()
            .ok_or(ExecutorClaimServiceError::MissingSettlementReceipt)?;
        log_event(
            "info",
            "executor_spend_settled",
            json!({
                "settlement_id": settlement_id.to_string(),
                "claim_id": state.claim.id.to_string(),
                "operation_key": state.claim.operation_key,
                "idempotent_replay": state.idempotent_replay,
                "spend_auth_token_id": state.token.id.to_string(),
                "decision_id": state.decision.id.to_string(),
                "hold_id": state.budget_hold.id.to_string(),
                "authorized_max_cents": receipt.authorized_max_cents,
                "actual_vendor_cost_cents": receipt.receipt.actual_vendor_cost_cents,
                "released_amount_cents": receipt.released_amount_cents,
                "provider_request_id": receipt.receipt.provider_request_id,
                "artifact_reference": receipt.receipt.artifact_reference,
                "merchant": state.decision.request.merchant,
                "task_id": state.decision.request.task_id,
                "reason": state.decision.request.reason,
            }),
        );
        Ok(state)
    }

    pub fn release<R>(
        &self,
        request: FinalizeExecutorClaimRequest,
        started_at: DateTime<Utc>,
        spend_manager: &mut SpendManager,
        budget_manager: &mut BudgetManager,
        repository: &mut R,
    ) -> Result<ExecutorClaimState, ExecutorClaimServiceError>
    where
        R: ExecutorClaimRepository,
    {
        let finalization = repository.release_executor_claim_transactionally(
            &request.owner_user_id,
            &request.agent_id,
            &request.operation_key,
            started_at,
        )?;
        let state = apply_finalization(finalization, spend_manager, budget_manager)?;
        log_event(
            "info",
            "executor_spend_released",
            json!({
                "spend_auth_token_id": state.token.id.to_string(),
                "claim_id": state.claim.id.to_string(),
                "operation_key": state.claim.operation_key,
                "idempotent_replay": state.idempotent_replay,
                "decision_id": state.decision.id.to_string(),
                "hold_id": state.budget_hold.id.to_string(),
                "amount_cents": state.budget_hold.amount_cents,
                "merchant": state.decision.request.merchant,
                "task_id": state.decision.request.task_id,
                "reason": state.decision.request.reason,
            }),
        );
        Ok(state)
    }

    pub fn reconcile<R>(
        &self,
        request: ReconcileExecutorClaimRequest,
        started_at: DateTime<Utc>,
        spend_manager: &mut SpendManager,
        budget_manager: &mut BudgetManager,
        repository: &mut R,
    ) -> Result<ExecutorClaimState, ExecutorClaimServiceError>
    where
        R: ExecutorClaimRepository,
    {
        let provider_reference = request.provider_reference.trim();
        if provider_reference.is_empty() {
            return Err(ExecutorClaimServiceError::EmptyProviderReference);
        }
        let evidence = request.evidence.trim();
        if evidence.is_empty() {
            return Err(ExecutorClaimServiceError::EmptyReconciliationEvidence);
        }

        let finalization = match request.outcome {
            ExecutorClaimReconciliationOutcome::VendorBilled => {
                let receipt = request
                    .receipt
                    .ok_or(ExecutorClaimServiceError::MissingSettlementReceipt)?;
                repository.reconcile_executor_claim_as_billed_transactionally(
                    &request.claim_id,
                    &request.owner_user_id,
                    provider_reference,
                    evidence,
                    PaymentId::new(),
                    receipt,
                    started_at,
                )?
            }
            ExecutorClaimReconciliationOutcome::VendorDidNotBill => {
                if request.receipt.is_some() {
                    return Err(ExecutorClaimServiceError::UnexpectedSettlementReceipt);
                }
                repository.reconcile_executor_claim_as_not_billed_transactionally(
                    &request.claim_id,
                    &request.owner_user_id,
                    provider_reference,
                    evidence,
                    started_at,
                )?
            }
        };
        let state = apply_finalization(finalization, spend_manager, budget_manager)?;
        log_event(
            "info",
            "executor_claim_reconciled",
            json!({
                "claim_id": state.claim.id.to_string(),
                "operation_key": state.claim.operation_key,
                "task_id": state.decision.request.task_id,
                "reason": state.decision.request.reason,
                "outcome": request.outcome.as_str(),
                "provider_reference": state.claim.provider_reference,
                "reconciled_by_user_id": request.owner_user_id.to_string(),
                "idempotent_replay": state.idempotent_replay,
                "hold_id": state.budget_hold.id.to_string(),
                "amount_cents": state.budget_hold.amount_cents,
            }),
        );
        Ok(state)
    }
}

fn apply_finalization(
    finalization: ExecutorFinalizationResult,
    spend_manager: &mut SpendManager,
    budget_manager: &mut BudgetManager,
) -> Result<ExecutorClaimState, ExecutorClaimServiceError> {
    spend_manager.apply_persisted_executor_finalization(
        finalization.claim.clone(),
        finalization.token.clone(),
    );
    budget_manager
        .apply_persisted_finalization(finalization.hold.clone(), finalization.balance.clone());
    claim_state_from_records(
        finalization.claim,
        ValidatedSpendAuthorization {
            spend_auth_token_id: finalization.token.id.clone(),
            owner_user_id: finalization.token.owner_user_id.clone(),
            spend_decision_id: finalization.token.spend_decision_id.clone(),
            expires_at: finalization.token.expires_at,
        },
        finalization.hold,
        finalization.balance,
        finalization.receipt,
        finalization.idempotent_replay,
        spend_manager,
    )
}

fn claim_state(
    claim: SpendExecutorClaimRecord,
    idempotent_replay: bool,
    spend_manager: &SpendManager,
    budget_manager: &BudgetManager,
) -> Result<ExecutorClaimState, ExecutorClaimServiceError> {
    let token = spend_manager
        .auth_token_record(&claim.spend_auth_token_id)
        .ok_or(ExecutorClaimServiceError::MissingClaimToken)?;
    let hold = budget_manager
        .get_budget_hold_by_spend_decision(&token.spend_decision_id)
        .ok_or(ExecutorClaimServiceError::MissingClaimBudgetHold)?;
    let balance = budget_manager
        .get_budget_balance(&hold.budget_id)
        .ok_or(ExecutorClaimServiceError::MissingClaimBudgetBalance)?;
    claim_state_from_records(
        claim,
        ValidatedSpendAuthorization {
            spend_auth_token_id: token.id.clone(),
            owner_user_id: token.owner_user_id.clone(),
            spend_decision_id: token.spend_decision_id.clone(),
            expires_at: token.expires_at,
        },
        hold,
        balance,
        None,
        idempotent_replay,
        spend_manager,
    )
}

fn claim_state_from_records(
    claim: SpendExecutorClaimRecord,
    authorization: ValidatedSpendAuthorization,
    budget_hold: BudgetHold,
    budget_balance: BudgetBalance,
    settlement_receipt: Option<PersistedSpendExecutorSettlementReceipt>,
    idempotent_replay: bool,
    spend_manager: &SpendManager,
) -> Result<ExecutorClaimState, ExecutorClaimServiceError> {
    let token = spend_manager
        .auth_token_record(&claim.spend_auth_token_id)
        .ok_or(ExecutorClaimServiceError::MissingClaimToken)?;
    let decision = spend_manager
        .decision_record(&token.spend_decision_id)
        .ok_or(ExecutorClaimServiceError::MissingClaimDecision)?;
    if decision.owner_user_id != claim.owner_user_id || token.owner_user_id != claim.owner_user_id {
        return Err(ExecutorClaimServiceError::ClaimOwnerMismatch);
    }
    if budget_hold.spend_decision_id != decision.id
        || budget_hold.executor_claim_id.as_ref() != Some(&claim.id)
    {
        return Err(ExecutorClaimServiceError::ClaimBudgetHoldMismatch);
    }

    Ok(ExecutorClaimState {
        claim,
        decision,
        token,
        authorization,
        budget_hold,
        budget_balance,
        settlement_receipt,
        idempotent_replay,
    })
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use hubu_common::{
        ids::AgentAccountId, models::UserContext, money::Currency, time::TimePeriod,
    };

    use super::*;
    use crate::{
        app::{
            ApprovedSpendAuthorization, AuthorizeSpendRequest, SpendApprovalService,
            SpendAuthorizationOutcome,
        },
        budget::{BudgetHoldStatus, CreateSingleBudgetRequest},
        persistence::{BudgetRepository, SpendRepository, SqliteGovernanceRepository},
        policy::{
            condition::{Condition, Field, PolicyValue},
            model::{Effect, Policy, Rule},
        },
        spend::{SpendExecutorPriceModelSnapshot, SpendExecutorSettlementReceipt},
    };

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

    struct ServiceHarness {
        service: ExecutorClaimService,
        spend_manager: SpendManager,
        budget_manager: BudgetManager,
        repository: SqliteGovernanceRepository,
        user: UserContext,
        agent_id: AgentId,
        account_id: AgentAccountId,
        authorization: ApprovedSpendAuthorization,
    }

    impl ServiceHarness {
        fn new() -> Self {
            let service = ExecutorClaimService;
            let mut spend_manager = SpendManager::new();
            let mut budget_manager = BudgetManager::new();
            let mut repository =
                SqliteGovernanceRepository::in_memory().expect("repository should initialize");
            let user = UserContext::new(UserId::new());
            let agent_id = AgentId::new();
            let account_id = AgentAccountId::new();
            let period = TimePeriod::new(
                Utc::now() - Duration::minutes(1),
                Some(Utc::now() + Duration::hours(2)),
            )
            .expect("period should be valid");
            let budget = budget_manager
                .create_single_budget(CreateSingleBudgetRequest {
                    agent_id: agent_id.clone(),
                    amount_limit_cents: 1_000,
                    currency: Currency::Usd,
                    period,
                })
                .expect("budget should create");
            repository
                .save_budget_with_balance(&budget.budget, &budget.balance)
                .expect("budget should persist");
            let policy = Policy {
                id: "claim-service-policy".to_string(),
                version: "v1".to_string(),
                owner_user_id: user.user_id.clone(),
                default_effect: Effect::NeedsApproval,
                rules: vec![Rule {
                    id: "allow-small-spend".to_string(),
                    effect: Effect::Allow,
                    when: Condition::Lte {
                        field: Field::Amount,
                        value: PolicyValue::MoneyCents(1_000),
                    },
                    reason: "within test limit".to_string(),
                }],
            };
            let authorization = match SpendApprovalService
                .authorize(
                    AuthorizeSpendRequest {
                        operation_key: "executor-operation".to_string(),
                        user: user.clone(),
                        agent_id: agent_id.clone(),
                        agent_account_id: account_id.clone(),
                        amount_cents: 500,
                        currency: Currency::Usd,
                        merchant: Some("vendor.example".to_string()),
                        execution_scope: None,
                        task_id: Some("task-123".to_string()),
                        reason: "Test executor authorization".to_string(),
                        workload_profile: "default".to_string(),
                    },
                    &policy,
                    &mut spend_manager,
                    &mut budget_manager,
                    &mut repository,
                )
                .expect("authorization should succeed")
            {
                SpendAuthorizationOutcome::Approved(authorization) => authorization,
                SpendAuthorizationOutcome::Rejected(rejection) => {
                    panic!("authorization should be approved: {:?}", rejection.reasons)
                }
            };

            Self {
                service,
                spend_manager,
                budget_manager,
                repository,
                user,
                agent_id,
                account_id,
                authorization,
            }
        }

        fn claim(&mut self) -> ExecutorClaimState {
            self.service
                .claim(
                    ClaimExecutorSpendRequest {
                        authorization: SpendPaymentValidationRequest {
                            spend_auth_token_id: self.authorization.token.id.clone(),
                            owner_user_id: self.user.user_id.clone(),
                            agent_id: self.agent_id.clone(),
                            agent_account_id: self.account_id.clone(),
                            amount_cents: self.authorization.amount_cents,
                            currency: self.authorization.currency,
                            merchant: self.authorization.merchant.clone(),
                            execution_scope: self.authorization.execution_scope.clone(),
                            task_id: self.authorization.task_id.clone(),
                        },
                        operation_key: self.authorization.operation_key.clone(),
                    },
                    &mut self.spend_manager,
                    &mut self.budget_manager,
                    &mut self.repository,
                )
                .expect("claim should succeed")
        }
    }

    #[test]
    fn claim_service_owns_claim_creation_lookup_and_executor_settlement() {
        let mut harness = ServiceHarness::new();
        let claim = harness.claim();

        assert!(matches!(
            claim.budget_hold.status,
            BudgetHoldStatus::Claimed
        ));
        assert_eq!(
            harness
                .repository
                .load_executor_claims()
                .expect("claims should load")
                .len(),
            1
        );
        let found = harness
            .service
            .get(
                &claim.claim.id,
                &harness.user.user_id,
                &harness.spend_manager,
                &harness.budget_manager,
            )
            .expect("claim should be found");
        assert_eq!(found.claim.id, claim.claim.id);

        let settled = harness
            .service
            .settle(
                SettleExecutorClaimRequest {
                    owner_user_id: harness.user.user_id.clone(),
                    agent_id: harness.agent_id.clone(),
                    operation_key: harness.authorization.operation_key.clone(),
                    receipt: settlement_receipt(400),
                },
                Utc::now(),
                &mut harness.spend_manager,
                &mut harness.budget_manager,
                &mut harness.repository,
            )
            .expect("active claim should settle");
        assert!(matches!(
            settled.claim.status,
            SpendExecutorClaimStatus::Settled
        ));
        assert!(settled.token.used_at.is_some());
        assert_eq!(settled.budget_balance.consumed_amount_cents, 400);
        assert_eq!(settled.budget_balance.remaining_amount_cents, 600);
        assert_eq!(
            settled
                .settlement_receipt
                .as_ref()
                .expect("receipt should be returned")
                .authorized_max_cents,
            500
        );
    }

    #[test]
    fn claim_service_lists_and_reconciles_expired_claims() {
        let mut harness = ServiceHarness::new();
        let claim = harness.claim();
        let after_expiry = claim.claim.expires_at + Duration::seconds(1);

        let pending = harness
            .service
            .list_requiring_reconciliation(
                &harness.user.user_id,
                after_expiry,
                &harness.spend_manager,
                &harness.budget_manager,
            )
            .expect("reconciliation queue should load");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].claim.id, claim.claim.id);

        let reconciled = harness
            .service
            .reconcile(
                ReconcileExecutorClaimRequest {
                    claim_id: claim.claim.id,
                    owner_user_id: harness.user.user_id.clone(),
                    provider_reference: " vendor-charge-123 ".to_string(),
                    evidence: " provider export confirms billing ".to_string(),
                    outcome: ExecutorClaimReconciliationOutcome::VendorBilled,
                    receipt: Some(settlement_receipt(400)),
                },
                after_expiry,
                &mut harness.spend_manager,
                &mut harness.budget_manager,
                &mut harness.repository,
            )
            .expect("expired claim should reconcile");
        assert!(matches!(
            reconciled.claim.status,
            SpendExecutorClaimStatus::Settled
        ));
        assert_eq!(
            reconciled.claim.provider_reference.as_deref(),
            Some("vendor-charge-123")
        );
        assert_eq!(
            reconciled.claim.reconciliation_evidence.as_deref(),
            Some("provider export confirms billing")
        );
        assert_eq!(reconciled.budget_balance.consumed_amount_cents, 400);
        assert_eq!(reconciled.budget_balance.remaining_amount_cents, 600);

        let pending = harness
            .service
            .list_requiring_reconciliation(
                &harness.user.user_id,
                after_expiry,
                &harness.spend_manager,
                &harness.budget_manager,
            )
            .expect("reconciliation queue should reload");
        assert!(pending.is_empty());
    }

    #[test]
    fn claim_service_reconciliation_can_release_unbilled_claim() {
        let mut harness = ServiceHarness::new();
        let claim = harness.claim();
        let after_expiry = claim.claim.expires_at + Duration::seconds(1);

        let reconciled = harness
            .service
            .reconcile(
                ReconcileExecutorClaimRequest {
                    claim_id: claim.claim.id,
                    owner_user_id: harness.user.user_id.clone(),
                    provider_reference: "vendor-search-456".to_string(),
                    evidence: "billing search found no charge".to_string(),
                    outcome: ExecutorClaimReconciliationOutcome::VendorDidNotBill,
                    receipt: None,
                },
                after_expiry,
                &mut harness.spend_manager,
                &mut harness.budget_manager,
                &mut harness.repository,
            )
            .expect("expired claim should release");
        assert!(matches!(
            reconciled.claim.status,
            SpendExecutorClaimStatus::Released
        ));
        assert!(reconciled.token.revoked_at.is_some());
        assert_eq!(reconciled.budget_balance.remaining_amount_cents, 1_000);
    }
}
