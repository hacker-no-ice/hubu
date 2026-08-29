use chrono::{DateTime, Utc};
use hubu_common::{
    execution_scope::ExecutionScope,
    ids::{AgentAccountId, AgentId, BudgetId, SpendAuthTokenId, SpendDecisionId},
    models::UserContext,
    money::Currency,
};
use hubu_wallet::{
    PaymentAttemptRepository, PaymentAttemptStorageError, PaymentDestination, PaymentError,
    PaymentManager, PaymentRail, PaymentRailKind, PaymentRequest, PaymentResponse, PaymentStatus,
    SpendAuthorizationValidator,
};
use serde_json::json;

use crate::{
    budget::{
        BudgetBalance, BudgetHold, BudgetManager, BudgetManagerError, ReleaseBudgetResponse,
        ReserveBudgetRequest, ReserveBudgetResponse, SettleBudgetResponse,
    },
    persistence::{BudgetRepository, SpendAttemptAdmission, SpendRepository},
    policy::model::{Effect, Policy},
    spend::{
        IssuedSpendAuthToken, SpendAuthTokenRecord, SpendAuthorizationDecision,
        SpendEvaluationResponse, SpendManager, SpendRequest, SpendRetryAction, SpendRetryGuidance,
    },
    storage::StorageError,
    telemetry::log_event,
};

pub struct SpendApprovalService;

#[derive(Debug, Clone)]
pub struct AuthorizeSpendRequest {
    pub operation_key: String,
    pub user: UserContext,
    pub agent_id: AgentId,
    pub agent_account_id: AgentAccountId,
    pub amount_cents: i64,
    pub currency: Currency,
    pub merchant: Option<String>,
    pub execution_scope: Option<ExecutionScope>,
    pub task_id: Option<String>,
    pub reason: String,
    pub lease_profile: String,
}

#[derive(Debug, Clone)]
pub enum SpendAuthorizationOutcome {
    Approved(ApprovedSpendAuthorization),
    Rejected(RejectedSpendAuthorization),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum HumanApprovalDecision {
    Approve,
    Deny,
}

#[derive(Debug, Clone)]
pub struct ApprovedSpendAuthorization {
    pub operation_key: String,
    pub user: UserContext,
    pub agent_id: AgentId,
    pub agent_account_id: AgentAccountId,
    pub amount_cents: i64,
    pub currency: Currency,
    pub merchant: Option<String>,
    pub execution_scope: Option<ExecutionScope>,
    pub task_id: Option<String>,
    pub reason: String,
    pub lease_profile: String,
    pub evaluation: SpendEvaluationResponse,
    pub token: IssuedSpendAuthToken,
    pub budget_reservation: ReserveBudgetResponse,
}

impl ApprovedSpendAuthorization {
    pub fn auth_token_id(&self) -> String {
        self.token.id.to_string()
    }
}

#[derive(Debug, Clone)]
pub struct RejectedSpendAuthorization {
    pub operation_key: String,
    pub user: UserContext,
    pub agent_id: AgentId,
    pub agent_account_id: AgentAccountId,
    pub amount_cents: i64,
    pub currency: Currency,
    pub merchant: Option<String>,
    pub execution_scope: Option<ExecutionScope>,
    pub task_id: Option<String>,
    pub reason: String,
    pub lease_profile: String,
    pub evaluation: SpendEvaluationResponse,
    pub decision: Effect,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SpendPaymentSpec {
    pub idempotency_key: String,
    pub rail: PaymentRailKind,
    pub destination: PaymentDestination,
    pub memo: Option<String>,
    /// Controls what happens when the payment rail returns a failed response.
    /// Hard payment manager errors still release holds because no retryable
    /// payment attempt was accepted.
    pub failed_payment_hold_policy: FailedPaymentHoldPolicy,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FailedPaymentHoldPolicy {
    Release,
    KeepFrozenForRetry,
}

#[derive(Debug, Clone)]
pub struct SpendPaymentSettlement {
    pub payment: PaymentResponse,
    pub budget_update: BudgetHoldUpdate,
}

#[derive(Debug, Clone)]
pub enum BudgetHoldUpdate {
    Settled(SettleBudgetResponse),
    Released(ReleaseBudgetResponse),
    Frozen(ReserveBudgetResponse),
}

impl BudgetHoldUpdate {
    pub fn hold_and_balance(&self) -> (&BudgetHold, &BudgetBalance) {
        match self {
            Self::Settled(response) => (&response.hold, &response.balance),
            Self::Released(response) => (&response.hold, &response.balance),
            Self::Frozen(response) => (&response.hold, &response.balance),
        }
    }

    fn persisted_hold_and_balance(&self) -> Option<(&BudgetHold, &BudgetBalance)> {
        match self {
            Self::Settled(response) => Some((&response.hold, &response.balance)),
            Self::Released(response) => Some((&response.hold, &response.balance)),
            Self::Frozen(_) => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SpendApprovalError {
    #[error("spend amount must be positive")]
    NonPositiveAmount,

    #[error("spend decision was not recorded")]
    MissingSpendDecision,

    #[error("allowed spend did not issue an auth token")]
    MissingSpendAuthToken,

    #[error("used spend auth token was not recorded")]
    UsedSpendAuthTokenMissing,

    #[error("no active USD budget found for agent")]
    MissingActiveBudget,

    #[error("changed scope retry is unsafe for operation key `{operation_key}`")]
    ChangedScopeRetryRejected { operation_key: String },

    #[error("exact replay is pending durable authorization completion for operation key `{operation_key}`")]
    PendingExactReplay { operation_key: String },

    #[error("unknown spend approval request")]
    UnknownApprovalRequest,

    #[error("spend approval request belongs to another owner")]
    ApprovalOwnerMismatch,

    #[error("spend approval request is already {current} and cannot be changed to {requested}")]
    ApprovalAlreadyResolved {
        current: &'static str,
        requested: &'static str,
    },

    #[error(transparent)]
    Spend(#[from] crate::spend::SpendError),

    #[error(transparent)]
    Budget(#[from] BudgetManagerError),

    #[error(transparent)]
    Storage(#[from] StorageError),

    #[error(transparent)]
    Payment(#[from] PaymentError),

    #[error(transparent)]
    PaymentAttempt(#[from] PaymentAttemptStorageError),
}

impl SpendApprovalService {
    pub fn authorize<G>(
        &self,
        request: AuthorizeSpendRequest,
        policy: &Policy,
        spend_manager: &mut SpendManager,
        budget_manager: &mut BudgetManager,
        governance: &mut G,
    ) -> Result<SpendAuthorizationOutcome, SpendApprovalError>
    where
        G: SpendRepository + BudgetRepository,
    {
        self.authorize_at(
            request,
            policy,
            spend_manager,
            budget_manager,
            governance,
            Utc::now(),
        )
    }

    pub fn authorize_at<G>(
        &self,
        request: AuthorizeSpendRequest,
        policy: &Policy,
        spend_manager: &mut SpendManager,
        budget_manager: &mut BudgetManager,
        governance: &mut G,
        now: DateTime<Utc>,
    ) -> Result<SpendAuthorizationOutcome, SpendApprovalError>
    where
        G: SpendRepository + BudgetRepository,
    {
        if request.amount_cents <= 0 {
            return Err(SpendApprovalError::NonPositiveAmount);
        }

        let spend_request = SpendRequest {
            amount_cents: request.amount_cents,
            currency: request.currency,
            owner_user_id: request.user.user_id.clone(),
            agent_id: request.agent_id.clone(),
            agent_account_id: request.agent_account_id.clone(),
            merchant: request.merchant.clone(),
            execution_scope: request.execution_scope.clone(),
            category: None,
            task_id: request.task_id.clone(),
            reason: request.reason.clone(),
            lease_profile: request.lease_profile.clone(),
        };
        let operation_key = request.operation_key.trim().to_string();
        let actor = request.user.user_id.to_string();
        let admission = governance.admit_spend_attempt(
            &request.user.user_id,
            &operation_key,
            &spend_request,
            &actor,
            Utc::now(),
        )?;
        let mut evaluation = match admission {
            SpendAttemptAdmission::Admitted { revision } => spend_manager
                .evaluate_spend_at_revision(
                    &request.user,
                    &operation_key,
                    spend_request,
                    policy,
                    revision,
                    &actor,
                )?,
            SpendAttemptAdmission::ExactReplay { .. } => {
                if !spend_manager.has_decision_for_scope(
                    &request.agent_id,
                    &operation_key,
                    &spend_request,
                ) {
                    return Err(SpendApprovalError::PendingExactReplay { operation_key });
                }
                spend_manager.evaluate_spend(
                    &request.user,
                    &operation_key,
                    spend_request,
                    policy,
                )?
            }
            SpendAttemptAdmission::ChangedScopeBlocked => {
                return Err(SpendApprovalError::ChangedScopeRetryRejected { operation_key });
            }
        };
        let decision_record = spend_manager
            .decision_record(&evaluation.decision_id)
            .ok_or(SpendApprovalError::MissingSpendDecision)?;
        let token_record = evaluation
            .auth_token
            .as_ref()
            .and_then(|token| spend_manager.auth_token_record(&token.id));

        if evaluation.idempotent_replay {
            attach_attempt_audit(
                governance,
                &mut evaluation,
                &request.agent_id,
                &operation_key,
                SpendRetryAction::ReplayExactly,
                "replay this exact immutable scope to recover the current authorization state",
            )?;
            let latest = evaluation
                .attempt_history
                .iter()
                .find(|attempt| attempt.revision == evaluation.revision)
                .map(|attempt| (attempt.final_decision.clone(), attempt.reasons.clone()));

            if evaluation.evaluation.decision == Effect::NeedsApproval {
                match latest {
                    Some((SpendAuthorizationDecision::Allowed, _)) => {
                        let existing_hold = budget_manager
                            .get_budget_hold_by_spend_decision(&evaluation.decision_id)
                            .ok_or(SpendApprovalError::MissingActiveBudget)?;
                        let token = evaluation
                            .auth_token
                            .clone()
                            .ok_or(SpendApprovalError::MissingSpendAuthToken)?;
                        let balance = budget_manager
                            .get_budget_balance(&existing_hold.budget_id)
                            .ok_or(SpendApprovalError::MissingActiveBudget)?;
                        return Ok(SpendAuthorizationOutcome::Approved(
                            ApprovedSpendAuthorization {
                                operation_key: evaluation.operation_key.clone(),
                                user: request.user,
                                agent_id: request.agent_id,
                                agent_account_id: request.agent_account_id,
                                amount_cents: request.amount_cents,
                                currency: request.currency,
                                merchant: request.merchant,
                                execution_scope: request.execution_scope,
                                task_id: request.task_id,
                                reason: request.reason,
                                lease_profile: request.lease_profile,
                                evaluation,
                                token,
                                budget_reservation: ReserveBudgetResponse {
                                    hold: existing_hold,
                                    balance,
                                },
                            },
                        ));
                    }
                    Some((SpendAuthorizationDecision::Denied, reasons)) => {
                        return Ok(SpendAuthorizationOutcome::Rejected(
                            RejectedSpendAuthorization {
                                operation_key: evaluation.operation_key.clone(),
                                user: request.user,
                                agent_id: request.agent_id,
                                agent_account_id: request.agent_account_id,
                                amount_cents: request.amount_cents,
                                currency: request.currency,
                                merchant: request.merchant,
                                execution_scope: request.execution_scope,
                                task_id: request.task_id,
                                reason: request.reason,
                                lease_profile: request.lease_profile,
                                decision: Effect::Deny,
                                reasons,
                                evaluation,
                            },
                        ));
                    }
                    _ => {
                        evaluation.retry_guidance = SpendRetryGuidance {
                            action: SpendRetryAction::ReplayExactly,
                            operation_key: operation_key.clone(),
                            message: "approval is pending; replay this exact request after the human decides"
                                .to_string(),
                        };
                        return Ok(SpendAuthorizationOutcome::Rejected(
                            RejectedSpendAuthorization {
                                operation_key: evaluation.operation_key.clone(),
                                user: request.user,
                                agent_id: request.agent_id,
                                agent_account_id: request.agent_account_id,
                                amount_cents: request.amount_cents,
                                currency: request.currency,
                                merchant: request.merchant,
                                execution_scope: request.execution_scope,
                                task_id: request.task_id,
                                reason: request.reason,
                                lease_profile: request.lease_profile,
                                decision: Effect::NeedsApproval,
                                reasons: evaluation.evaluation.reasons.clone(),
                                evaluation,
                            },
                        ));
                    }
                }
            }

            if evaluation.evaluation.decision != Effect::Allow {
                attach_attempt_audit(
                    governance,
                    &mut evaluation,
                    &request.agent_id,
                    &operation_key,
                    SpendRetryAction::ReuseOperationKey,
                    "the prior attempt was denied; reuse this operation key with corrected scope",
                )?;
                return Ok(SpendAuthorizationOutcome::Rejected(
                    RejectedSpendAuthorization {
                        operation_key: evaluation.operation_key.clone(),
                        user: request.user,
                        agent_id: request.agent_id,
                        agent_account_id: request.agent_account_id,
                        amount_cents: request.amount_cents,
                        currency: request.currency,
                        merchant: request.merchant,
                        execution_scope: request.execution_scope,
                        task_id: request.task_id,
                        reason: request.reason,
                        lease_profile: request.lease_profile,
                        decision: evaluation.evaluation.decision,
                        reasons: evaluation.evaluation.reasons.clone(),
                        evaluation,
                    },
                ));
            }

            let existing_hold =
                budget_manager.get_budget_hold_by_spend_decision(&evaluation.decision_id);
            if let (Some(token), Some(hold)) = (evaluation.auth_token.clone(), existing_hold) {
                attach_attempt_audit(
                    governance,
                    &mut evaluation,
                    &request.agent_id,
                    &operation_key,
                    SpendRetryAction::ReplayExactly,
                    "replay this exact immutable scope to recover the original authorization",
                )?;
                let balance = budget_manager
                    .get_budget_balance(&hold.budget_id)
                    .ok_or(SpendApprovalError::MissingActiveBudget)?;
                return Ok(SpendAuthorizationOutcome::Approved(
                    ApprovedSpendAuthorization {
                        operation_key: evaluation.operation_key.clone(),
                        user: request.user,
                        agent_id: request.agent_id,
                        agent_account_id: request.agent_account_id,
                        amount_cents: request.amount_cents,
                        currency: request.currency,
                        merchant: request.merchant,
                        execution_scope: request.execution_scope,
                        task_id: request.task_id,
                        reason: request.reason,
                        lease_profile: request.lease_profile,
                        evaluation,
                        token,
                        budget_reservation: ReserveBudgetResponse { hold, balance },
                    },
                ));
            }

            attach_attempt_audit(
                governance,
                &mut evaluation,
                &request.agent_id,
                &operation_key,
                SpendRetryAction::ReuseOperationKey,
                "the prior attempt was denied; reuse this operation key with corrected scope",
            )?;
            return Ok(SpendAuthorizationOutcome::Rejected(budget_rejection(
                request,
                evaluation,
                "budget does not have enough remaining balance",
            )));
        }

        governance.save_spend_decision(&decision_record)?;

        if evaluation.evaluation.decision != Effect::Allow {
            if evaluation.evaluation.decision == Effect::Deny {
                governance.record_spend_attempt_outcome(
                    &decision_record,
                    SpendAuthorizationDecision::Denied,
                    &evaluation.evaluation.reasons,
                    Utc::now(),
                )?;
                attach_attempt_audit(
                    governance,
                    &mut evaluation,
                    &request.agent_id,
                    &operation_key,
                    SpendRetryAction::ReuseOperationKey,
                    "this attempt was denied; reuse this operation key with corrected scope",
                )?;
            } else {
                attach_attempt_audit(
                    governance,
                    &mut evaluation,
                    &request.agent_id,
                    &operation_key,
                    SpendRetryAction::ReplayExactly,
                    "approval is pending; replay this exact request after the human decides",
                )?;
            }
            return Ok(SpendAuthorizationOutcome::Rejected(
                RejectedSpendAuthorization {
                    operation_key: evaluation.operation_key.clone(),
                    user: request.user,
                    agent_id: request.agent_id,
                    agent_account_id: request.agent_account_id,
                    amount_cents: request.amount_cents,
                    currency: request.currency,
                    merchant: request.merchant,
                    execution_scope: request.execution_scope,
                    task_id: request.task_id,
                    reason: request.reason,
                    lease_profile: request.lease_profile,
                    decision: evaluation.evaluation.decision,
                    reasons: evaluation.evaluation.reasons.clone(),
                    evaluation,
                },
            ));
        }

        let token = evaluation
            .auth_token
            .clone()
            .ok_or(SpendApprovalError::MissingSpendAuthToken)?;
        let budget_id = match active_budget_id_for_spend(budget_manager, &request.agent_id, now) {
            Ok(budget_id) => budget_id,
            Err(SpendApprovalError::MissingActiveBudget) => {
                spend_manager.discard_auth_token_for_decision(&evaluation.decision_id);
                let reasons = vec!["no active USD budget found for agent".to_string()];
                governance.record_spend_attempt_outcome(
                    &decision_record,
                    SpendAuthorizationDecision::Denied,
                    &reasons,
                    Utc::now(),
                )?;
                attach_attempt_audit(
                    governance,
                    &mut evaluation,
                    &request.agent_id,
                    &operation_key,
                    SpendRetryAction::ReuseOperationKey,
                    "this attempt was denied; reuse this operation key with corrected scope",
                )?;
                return Ok(SpendAuthorizationOutcome::Rejected(budget_rejection(
                    request,
                    evaluation,
                    "no active USD budget found for agent",
                )));
            }
            Err(error) => return Err(error),
        };

        let budget_reservation = match budget_manager.reserve_budget_at(
            ReserveBudgetRequest {
                budget_id: budget_id.clone(),
                spend_decision_id: evaluation.decision_id.clone(),
                amount_cents: request.amount_cents,
                currency: request.currency,
                expires_at: token.expires_at,
            },
            now,
        ) {
            Ok(reservation) => reservation,
            Err(BudgetManagerError::InsufficientRemainingBudget) => {
                spend_manager.discard_auth_token_for_decision(&evaluation.decision_id);
                let reasons = vec!["budget does not have enough remaining balance".to_string()];
                governance.record_spend_attempt_outcome(
                    &decision_record,
                    SpendAuthorizationDecision::Denied,
                    &reasons,
                    Utc::now(),
                )?;
                attach_attempt_audit(
                    governance,
                    &mut evaluation,
                    &request.agent_id,
                    &operation_key,
                    SpendRetryAction::ReuseOperationKey,
                    "this attempt was denied; reuse this operation key with corrected scope",
                )?;
                log_event(
                    "warn",
                    "spend_budget_denied",
                    json!({
                        "agent_id": request.agent_id.to_string(),
                        "user_id": request.user.user_id.to_string(),
                        "decision_id": evaluation.decision_id.to_string(),
                        "budget_id": budget_id.to_string(),
                        "amount_cents": request.amount_cents,
                        "reason": "insufficient_remaining_budget",
                    }),
                );
                return Ok(SpendAuthorizationOutcome::Rejected(budget_rejection(
                    request,
                    evaluation,
                    "budget does not have enough remaining balance",
                )));
            }
            Err(error) => return Err(error.into()),
        };

        if let Some(token_record) = &token_record {
            if let Err(error) = governance.save_spend_auth_token(token_record) {
                let budget_release = budget_manager.release_budget(&budget_reservation.hold.id)?;
                log_event(
                    "warn",
                    "spend_budget_reservation_released",
                    json!({
                        "decision_id": evaluation.decision_id.to_string(),
                        "hold_id": budget_release.hold.id.to_string(),
                        "remaining_amount_cents": budget_release.balance.remaining_amount_cents,
                        "error": error.to_string(),
                    }),
                );
                return Err(error.into());
            }
        }
        governance.save_budget_hold(&budget_reservation.hold, &budget_reservation.balance)?;
        governance.record_spend_attempt_outcome(
            &decision_record,
            SpendAuthorizationDecision::Allowed,
            &evaluation.evaluation.reasons,
            Utc::now(),
        )?;
        attach_attempt_audit(
            governance,
            &mut evaluation,
            &request.agent_id,
            &operation_key,
            SpendRetryAction::ReplayExactly,
            "replay this exact immutable scope to recover the original authorization",
        )?;

        log_event(
            "info",
            "spend_authorization_reserved",
            json!({
                "operation_key": evaluation.operation_key,
                "budget_id": budget_reservation.hold.budget_id.to_string(),
                "hold_id": budget_reservation.hold.id.to_string(),
                "decision_id": evaluation.decision_id.to_string(),
                "amount_cents": budget_reservation.hold.amount_cents,
                "remaining_amount_cents": budget_reservation.balance.remaining_amount_cents,
                "frozen_amount_cents": budget_reservation.balance.frozen_amount_cents,
            }),
        );

        Ok(SpendAuthorizationOutcome::Approved(
            ApprovedSpendAuthorization {
                operation_key: evaluation.operation_key.clone(),
                user: request.user,
                agent_id: request.agent_id,
                agent_account_id: request.agent_account_id,
                amount_cents: request.amount_cents,
                currency: request.currency,
                merchant: request.merchant,
                execution_scope: request.execution_scope,
                task_id: request.task_id,
                reason: request.reason,
                lease_profile: request.lease_profile,
                evaluation,
                token,
                budget_reservation,
            },
        ))
    }

    pub fn resolve_human_approval<G>(
        &self,
        user: UserContext,
        approval_request_id: &SpendDecisionId,
        decision: HumanApprovalDecision,
        spend_manager: &mut SpendManager,
        budget_manager: &mut BudgetManager,
        governance: &mut G,
    ) -> Result<SpendAuthorizationOutcome, SpendApprovalError>
    where
        G: SpendRepository + BudgetRepository,
    {
        self.resolve_human_approval_at(
            user,
            approval_request_id,
            decision,
            spend_manager,
            budget_manager,
            governance,
            Utc::now(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn resolve_human_approval_at<G>(
        &self,
        user: UserContext,
        approval_request_id: &SpendDecisionId,
        decision: HumanApprovalDecision,
        spend_manager: &mut SpendManager,
        budget_manager: &mut BudgetManager,
        governance: &mut G,
        now: DateTime<Utc>,
    ) -> Result<SpendAuthorizationOutcome, SpendApprovalError>
    where
        G: SpendRepository + BudgetRepository,
    {
        let decision_record = spend_manager
            .decision_record(approval_request_id)
            .ok_or(SpendApprovalError::UnknownApprovalRequest)?;
        if decision_record.owner_user_id != user.user_id {
            return Err(SpendApprovalError::ApprovalOwnerMismatch);
        }
        if decision_record.evaluation.decision != Effect::NeedsApproval {
            return Err(SpendApprovalError::UnknownApprovalRequest);
        }

        let request = authorize_request_from_decision(user, &decision_record);
        let history = governance.load_spend_attempt_history(
            &decision_record.request.agent_id,
            &decision_record.operation_key,
        )?;
        let current = history
            .iter()
            .find(|attempt| attempt.revision == decision_record.revision)
            .map(|attempt| attempt.final_decision.clone())
            .unwrap_or(SpendAuthorizationDecision::PendingApproval);

        match (current, decision) {
            (SpendAuthorizationDecision::Allowed, HumanApprovalDecision::Deny) => {
                Err(SpendApprovalError::ApprovalAlreadyResolved {
                    current: "approved",
                    requested: "denied",
                })
            }
            (SpendAuthorizationDecision::Denied, HumanApprovalDecision::Approve) => {
                Err(SpendApprovalError::ApprovalAlreadyResolved {
                    current: "denied",
                    requested: "approved",
                })
            }
            (SpendAuthorizationDecision::Allowed, HumanApprovalDecision::Approve)
            | (SpendAuthorizationDecision::Denied, HumanApprovalDecision::Deny) => self
                .authorize_at(
                    request,
                    &Policy {
                        id: decision_record.evaluation.policy_id.clone(),
                        version: decision_record.evaluation.policy_version.clone(),
                        owner_user_id: decision_record.owner_user_id,
                        rules: Vec::new(),
                        default_effect: decision_record.evaluation.decision,
                    },
                    spend_manager,
                    budget_manager,
                    governance,
                    now,
                ),
            (SpendAuthorizationDecision::PendingApproval, HumanApprovalDecision::Deny) => {
                let reasons = vec!["human denied the immutable spend request".to_string()];
                governance.record_spend_attempt_outcome(
                    &decision_record,
                    SpendAuthorizationDecision::Denied,
                    &reasons,
                    Utc::now(),
                )?;
                let mut evaluation = evaluation_response_for_decision(
                    &decision_record,
                    None,
                    false,
                    SpendRetryAction::ReplayExactly,
                    "the human denied this immutable request",
                );
                attach_attempt_audit(
                    governance,
                    &mut evaluation,
                    &decision_record.request.agent_id,
                    &decision_record.operation_key,
                    SpendRetryAction::ReplayExactly,
                    "the human denied this immutable request",
                )?;
                Ok(SpendAuthorizationOutcome::Rejected(rejected_from_request(
                    request,
                    evaluation,
                    Effect::Deny,
                    reasons,
                )))
            }
            (SpendAuthorizationDecision::PendingApproval, HumanApprovalDecision::Approve) => {
                let token =
                    spend_manager.issue_auth_token_for_approved_decision(approval_request_id)?;
                let token_record = spend_manager
                    .auth_token_record(&token.id)
                    .ok_or(SpendApprovalError::MissingSpendAuthToken)?;
                let budget_id = active_budget_id_for_spend(
                    budget_manager,
                    &decision_record.request.agent_id,
                    now,
                )?;
                let budget_reservation = match budget_manager.reserve_budget_at(
                    ReserveBudgetRequest {
                        budget_id,
                        spend_decision_id: approval_request_id.clone(),
                        amount_cents: decision_record.request.amount_cents,
                        currency: decision_record.request.currency,
                        expires_at: token.expires_at,
                    },
                    now,
                ) {
                    Ok(reservation) => reservation,
                    Err(error) => {
                        spend_manager.discard_auth_token_for_decision(approval_request_id);
                        return Err(error.into());
                    }
                };

                let reasons = vec!["human approved the immutable spend request".to_string()];
                if let Err(error) = governance.save_approved_spend_transition(
                    &decision_record,
                    &token_record,
                    &budget_reservation.hold,
                    &budget_reservation.balance,
                    &reasons,
                    Utc::now(),
                ) {
                    let _ = budget_manager.release_budget(&budget_reservation.hold.id);
                    spend_manager.discard_auth_token_for_decision(approval_request_id);
                    return Err(error.into());
                }
                let mut evaluation = evaluation_response_for_decision(
                    &decision_record,
                    Some(token.clone()),
                    false,
                    SpendRetryAction::ReplayExactly,
                    "the human approved this request; replay returns the authorization",
                );
                attach_attempt_audit(
                    governance,
                    &mut evaluation,
                    &decision_record.request.agent_id,
                    &decision_record.operation_key,
                    SpendRetryAction::ReplayExactly,
                    "the human approved this request; replay returns the authorization",
                )?;
                Ok(SpendAuthorizationOutcome::Approved(
                    ApprovedSpendAuthorization {
                        operation_key: request.operation_key,
                        user: request.user,
                        agent_id: request.agent_id,
                        agent_account_id: request.agent_account_id,
                        amount_cents: request.amount_cents,
                        currency: request.currency,
                        merchant: request.merchant,
                        execution_scope: request.execution_scope,
                        task_id: request.task_id,
                        reason: request.reason,
                        lease_profile: request.lease_profile,
                        evaluation,
                        token,
                        budget_reservation,
                    },
                ))
            }
        }
    }

    // This orchestration boundary receives each stateful collaborator explicitly so callers
    // retain control of locking, persistence, and token loading.
    #[allow(clippy::too_many_arguments)]
    pub fn submit_payment<R, A, Attempts, Governance, LoadUsedToken>(
        &self,
        authorization: &ApprovedSpendAuthorization,
        payment_spec: SpendPaymentSpec,
        payment_manager: &mut PaymentManager<R, A>,
        payment_attempts: &mut Attempts,
        budget_manager: &mut BudgetManager,
        governance: &mut Governance,
        load_used_token: LoadUsedToken,
    ) -> Result<SpendPaymentSettlement, SpendApprovalError>
    where
        R: PaymentRail,
        A: SpendAuthorizationValidator,
        Attempts: PaymentAttemptRepository,
        Governance: SpendRepository + BudgetRepository,
        LoadUsedToken:
            FnOnce(&SpendAuthTokenId) -> Result<SpendAuthTokenRecord, SpendApprovalError>,
    {
        let payment_request = PaymentRequest {
            idempotency_key: payment_spec.idempotency_key,
            spend_auth_token_id: authorization.token.id.clone(),
            owner_user_id: authorization.user.user_id.clone(),
            agent_id: authorization.agent_id.clone(),
            agent_account_id: authorization.agent_account_id.clone(),
            amount_cents: authorization.amount_cents,
            currency: authorization.currency,
            merchant: authorization.merchant.clone(),
            execution_scope: authorization.execution_scope.clone(),
            task_id: authorization.task_id.clone(),
            rail: payment_spec.rail,
            destination: payment_spec.destination,
            memo: payment_spec.memo,
        };

        let payment_audit_request = payment_request.clone();
        let payment = match payment_manager.submit_payment(payment_request) {
            Ok(payment) => payment,
            Err(error) => {
                if !matches!(
                    authorization.budget_reservation.hold.status,
                    crate::budget::BudgetHoldStatus::Frozen
                ) {
                    return Err(error.into());
                }
                let budget_release =
                    release_authorized_hold(budget_manager, &authorization.budget_reservation)?;
                governance.update_budget_hold(&budget_release.hold, &budget_release.balance)?;
                log_event(
                    "warn",
                    "payment_failed_budget_released",
                    json!({
                        "decision_id": authorization.evaluation.decision_id.to_string(),
                        "hold_id": budget_release.hold.id.to_string(),
                        "error": error.to_string(),
                    }),
                );
                return Err(error.into());
            }
        };

        payment_attempts.save_payment_attempt(&payment_audit_request, &payment)?;

        if payment.status == PaymentStatus::Succeeded {
            let used_token = load_used_token(&payment_audit_request.spend_auth_token_id)?;
            governance.update_spend_auth_token(&used_token)?;
        }

        let budget_update = if payment.status == PaymentStatus::Succeeded {
            if matches!(
                authorization.budget_reservation.hold.status,
                crate::budget::BudgetHoldStatus::Settled
            ) {
                BudgetHoldUpdate::Settled(SettleBudgetResponse {
                    hold: authorization.budget_reservation.hold.clone(),
                    balance: authorization.budget_reservation.balance.clone(),
                })
            } else {
                let budget_settlement =
                    budget_manager.settle_budget(&authorization.budget_reservation.hold.id)?;
                BudgetHoldUpdate::Settled(budget_settlement)
            }
        } else if payment_spec.failed_payment_hold_policy == FailedPaymentHoldPolicy::Release {
            if matches!(
                authorization.budget_reservation.hold.status,
                crate::budget::BudgetHoldStatus::Released
            ) {
                BudgetHoldUpdate::Released(ReleaseBudgetResponse {
                    hold: authorization.budget_reservation.hold.clone(),
                    balance: authorization.budget_reservation.balance.clone(),
                })
            } else {
                let budget_release =
                    release_authorized_hold(budget_manager, &authorization.budget_reservation)?;
                BudgetHoldUpdate::Released(budget_release)
            }
        } else {
            BudgetHoldUpdate::Frozen(authorization.budget_reservation.clone())
        };
        if let Some(update) = budget_update.persisted_hold_and_balance() {
            governance.update_budget_hold(update.0, update.1)?;
        }

        log_event(
            "info",
            "payment_submitted_for_spend",
            json!({
                "decision_id": authorization.evaluation.decision_id.to_string(),
                "payment_id": payment.payment_id.to_string(),
                "payment_status": payment_status_name(payment.status),
                "ledger_transaction_id": payment.ledger_transaction_id.as_ref().map(ToString::to_string),
                "rail_reference": payment.rail_reference,
                "failure_reason": payment.failure_reason,
            }),
        );

        Ok(SpendPaymentSettlement {
            payment,
            budget_update,
        })
    }
}

fn authorize_request_from_decision(
    user: UserContext,
    decision: &crate::spend::SpendDecisionRecord,
) -> AuthorizeSpendRequest {
    AuthorizeSpendRequest {
        operation_key: decision.operation_key.clone(),
        user,
        agent_id: decision.request.agent_id.clone(),
        agent_account_id: decision.request.agent_account_id.clone(),
        amount_cents: decision.request.amount_cents,
        currency: decision.request.currency,
        merchant: decision.request.merchant.clone(),
        execution_scope: decision.request.execution_scope.clone(),
        task_id: decision.request.task_id.clone(),
        reason: decision.request.reason.clone(),
        lease_profile: decision.request.lease_profile.clone(),
    }
}

fn evaluation_response_for_decision(
    decision: &crate::spend::SpendDecisionRecord,
    auth_token: Option<IssuedSpendAuthToken>,
    idempotent_replay: bool,
    action: SpendRetryAction,
    message: &str,
) -> SpendEvaluationResponse {
    SpendEvaluationResponse {
        operation_key: decision.operation_key.clone(),
        decision_id: decision.id.clone(),
        evaluation: decision.evaluation.clone(),
        auth_token,
        idempotent_replay,
        revision: decision.revision,
        retry_guidance: SpendRetryGuidance {
            action,
            operation_key: decision.operation_key.clone(),
            message: message.to_string(),
        },
        attempt_history: Vec::new(),
    }
}

fn rejected_from_request(
    request: AuthorizeSpendRequest,
    evaluation: SpendEvaluationResponse,
    decision: Effect,
    reasons: Vec<String>,
) -> RejectedSpendAuthorization {
    RejectedSpendAuthorization {
        operation_key: request.operation_key,
        user: request.user,
        agent_id: request.agent_id,
        agent_account_id: request.agent_account_id,
        amount_cents: request.amount_cents,
        currency: request.currency,
        merchant: request.merchant,
        execution_scope: request.execution_scope,
        task_id: request.task_id,
        reason: request.reason,
        lease_profile: request.lease_profile,
        evaluation,
        decision,
        reasons,
    }
}

fn budget_rejection(
    request: AuthorizeSpendRequest,
    evaluation: SpendEvaluationResponse,
    reason: &str,
) -> RejectedSpendAuthorization {
    RejectedSpendAuthorization {
        operation_key: evaluation.operation_key.clone(),
        user: request.user,
        agent_id: request.agent_id,
        agent_account_id: request.agent_account_id,
        amount_cents: request.amount_cents,
        currency: request.currency,
        merchant: request.merchant,
        execution_scope: request.execution_scope,
        task_id: request.task_id,
        reason: request.reason,
        lease_profile: request.lease_profile,
        evaluation,
        decision: Effect::Deny,
        reasons: vec![reason.to_string()],
    }
}

fn attach_attempt_audit<G: SpendRepository>(
    governance: &G,
    evaluation: &mut SpendEvaluationResponse,
    agent_id: &AgentId,
    operation_key: &str,
    action: SpendRetryAction,
    message: &str,
) -> Result<(), StorageError> {
    evaluation.attempt_history = governance.load_spend_attempt_history(agent_id, operation_key)?;
    let (action, message) = if action == SpendRetryAction::ReuseOperationKey
        && !governance.changed_scope_retry_is_safe(agent_id, operation_key)?
    {
        if evaluation.idempotent_replay {
            (
                SpendRetryAction::ReplayExactly,
                "replay this exact immutable scope; changed scope requires a new operation key",
            )
        } else {
            (
                SpendRetryAction::CreateNewOperation,
                "this operation has side-effect-capable history; create a new operation key",
            )
        }
    } else {
        (action, message)
    };
    evaluation.retry_guidance = SpendRetryGuidance {
        action,
        operation_key: operation_key.to_string(),
        message: message.to_string(),
    };
    Ok(())
}

fn release_authorized_hold(
    budget_manager: &mut BudgetManager,
    budget_reservation: &ReserveBudgetResponse,
) -> Result<ReleaseBudgetResponse, BudgetManagerError> {
    budget_manager.release_budget(&budget_reservation.hold.id)
}

fn active_budget_id_for_spend(
    budget_manager: &BudgetManager,
    agent_id: &AgentId,
    now: DateTime<Utc>,
) -> Result<BudgetId, SpendApprovalError> {
    budget_manager
        .available_budget_id_for_agent_at(agent_id, Currency::Usd, now)?
        .ok_or(SpendApprovalError::MissingActiveBudget)
}

fn payment_status_name(status: PaymentStatus) -> &'static str {
    match status {
        PaymentStatus::Succeeded => "succeeded",
        PaymentStatus::Failed => "failed",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::Duration;
    use hubu_common::ids::UserId;
    use hubu_common::time::TimePeriod;
    use hubu_wallet::{
        MockPaymentRail, PaymentAttemptRecord, SqliteLedger, ValidatedSpendAuthorization,
    };

    use super::*;
    use crate::{
        budget::CreateSingleBudgetRequest,
        persistence::SqliteGovernanceRepository,
        policy::{
            condition::{Condition, Field, PolicyValue},
            model::Rule,
        },
        spend::SpendPaymentValidationRequest,
    };

    #[derive(Default)]
    struct InMemoryPaymentAttempts {
        attempts: Vec<PaymentAttemptRecord>,
    }

    impl PaymentAttemptRepository for InMemoryPaymentAttempts {
        fn save_payment_attempt(
            &mut self,
            request: &PaymentRequest,
            response: &PaymentResponse,
        ) -> Result<(), PaymentAttemptStorageError> {
            self.attempts.push(PaymentAttemptRecord {
                payment_id: response.payment_id.clone(),
                idempotency_key: request.idempotency_key.clone(),
                spend_auth_token_id: request.spend_auth_token_id.clone(),
                owner_user_id: request.owner_user_id.clone(),
                agent_id: request.agent_id.clone(),
                agent_account_id: request.agent_account_id.clone(),
                amount_cents: request.amount_cents,
                currency: request.currency,
                merchant: request.merchant.clone(),
                execution_scope: request.execution_scope.clone(),
                task_id: request.task_id.clone(),
                rail: request.rail,
                destination: request.destination.clone(),
                memo: request.memo.clone(),
                status: response.status,
                ledger_transaction_id: response.ledger_transaction_id.clone(),
                rail_reference: response.rail_reference.clone(),
                failure_reason: response.failure_reason.clone(),
                created_at: response.created_at,
            });
            Ok(())
        }

        fn list_payment_attempts(
            &self,
        ) -> Result<Vec<PaymentAttemptRecord>, PaymentAttemptStorageError> {
            Ok(self.attempts.clone())
        }
    }

    #[derive(Clone)]
    struct SharedTestAuthorizer {
        spend_manager: Arc<Mutex<SpendManager>>,
    }

    impl SpendAuthorizationValidator for SharedTestAuthorizer {
        fn validate_payment_request(
            &self,
            request: &PaymentRequest,
        ) -> Result<ValidatedSpendAuthorization, PaymentError> {
            self.spend_manager
                .lock()
                .map_err(|_| PaymentError::AuthorizationRejected {
                    reason: "spend manager lock poisoned".to_string(),
                })?
                .validate_auth_token_for_payment(&SpendPaymentValidationRequest {
                    spend_auth_token_id: request.spend_auth_token_id.clone(),
                    owner_user_id: request.owner_user_id.clone(),
                    agent_id: request.agent_id.clone(),
                    agent_account_id: request.agent_account_id.clone(),
                    amount_cents: request.amount_cents,
                    currency: request.currency,
                    merchant: request.merchant.clone(),
                    execution_scope: request.execution_scope.clone(),
                    task_id: request.task_id.clone(),
                })
                .map(|validation| ValidatedSpendAuthorization {
                    spend_auth_token_id: validation.spend_auth_token_id,
                    owner_user_id: validation.owner_user_id,
                })
                .map_err(|error| PaymentError::AuthorizationRejected {
                    reason: error.to_string(),
                })
        }

        fn mark_token_used(
            &mut self,
            token_id: &SpendAuthTokenId,
            payment_id: &hubu_common::ids::PaymentId,
        ) -> Result<(), PaymentError> {
            self.spend_manager
                .lock()
                .map_err(|_| PaymentError::AuthorizationRejected {
                    reason: "spend manager lock poisoned".to_string(),
                })?
                .mark_auth_token_used(token_id, payment_id.clone())
                .map_err(|error| PaymentError::AuthorizationRejected {
                    reason: error.to_string(),
                })
        }
    }

    struct ServiceHarness {
        service: SpendApprovalService,
        spend_manager: Arc<Mutex<SpendManager>>,
        budget_manager: BudgetManager,
        governance: SqliteGovernanceRepository,
        payment_attempts: InMemoryPaymentAttempts,
        payment_manager: PaymentManager<MockPaymentRail, SharedTestAuthorizer>,
        user: UserContext,
        agent_id: AgentId,
        account_id: AgentAccountId,
        policy: Policy,
    }

    impl ServiceHarness {
        fn new(budget_cents: i64) -> Self {
            let service = SpendApprovalService;
            let spend_manager = Arc::new(Mutex::new(SpendManager::new()));
            let user = UserContext::new(UserId::new());
            let agent_id = AgentId::new();
            let account_id = AgentAccountId::new();
            let mut budget_manager = BudgetManager::new();
            let mut governance =
                SqliteGovernanceRepository::in_memory().expect("governance should initialize");
            let period = TimePeriod::new(
                Utc::now() - Duration::minutes(1),
                Some(Utc::now() + Duration::hours(1)),
            )
            .expect("period should be valid");

            let budget = budget_manager
                .create_single_budget(CreateSingleBudgetRequest {
                    agent_id: agent_id.clone(),
                    amount_limit_cents: budget_cents,
                    currency: Currency::Usd,
                    period,
                })
                .expect("budget should create");
            governance
                .save_budget_with_balance(&budget.budget, &budget.version, &budget.balance)
                .expect("budget should persist");

            let payment_manager = PaymentManager::new(
                user.user_id.clone(),
                MockPaymentRail,
                SharedTestAuthorizer {
                    spend_manager: Arc::clone(&spend_manager),
                },
                SqliteLedger::in_memory().expect("ledger should initialize"),
            )
            .expect("payment manager should initialize");
            let policy = allow_policy(user.user_id.clone());

            Self {
                service,
                spend_manager,
                budget_manager,
                governance,
                payment_attempts: InMemoryPaymentAttempts::default(),
                payment_manager,
                user,
                agent_id,
                account_id,
                policy,
            }
        }

        fn authorize(&mut self, amount_cents: i64, merchant: &str) -> SpendAuthorizationOutcome {
            let mut spend_manager = self
                .spend_manager
                .lock()
                .expect("spend manager lock should not poison");
            self.service
                .authorize(
                    AuthorizeSpendRequest {
                        operation_key: "test-operation".to_string(),
                        user: self.user.clone(),
                        agent_id: self.agent_id.clone(),
                        agent_account_id: self.account_id.clone(),
                        amount_cents,
                        currency: Currency::Usd,
                        merchant: Some(merchant.to_string()),
                        execution_scope: None,
                        task_id: Some("task-123".to_string()),
                        reason: "Test authorization".to_string(),
                        lease_profile: "default".to_string(),
                    },
                    &self.policy,
                    &mut spend_manager,
                    &mut self.budget_manager,
                    &mut self.governance,
                )
                .expect("authorization should not error")
        }

        fn submit_payment(
            &mut self,
            authorization: &ApprovedSpendAuthorization,
        ) -> SpendPaymentSettlement {
            self.submit_payment_with_hold_policy(authorization, FailedPaymentHoldPolicy::Release)
        }

        fn submit_payment_with_hold_policy(
            &mut self,
            authorization: &ApprovedSpendAuthorization,
            failed_payment_hold_policy: FailedPaymentHoldPolicy,
        ) -> SpendPaymentSettlement {
            let spend_manager = Arc::clone(&self.spend_manager);
            self.service
                .submit_payment(
                    authorization,
                    SpendPaymentSpec {
                        idempotency_key: format!(
                            "{}:{}",
                            authorization.evaluation.decision_id,
                            authorization.task_id.as_deref().unwrap_or_default()
                        ),
                        rail: PaymentRailKind::FiatMock,
                        destination: PaymentDestination::FiatAccount {
                            account_ref: "local-merchant-account".to_string(),
                        },
                        memo: Some("Hubu mock payment".to_string()),
                        failed_payment_hold_policy,
                    },
                    &mut self.payment_manager,
                    &mut self.payment_attempts,
                    &mut self.budget_manager,
                    &mut self.governance,
                    move |token_id| {
                        spend_manager
                            .lock()
                            .map_err(|_| SpendApprovalError::UsedSpendAuthTokenMissing)?
                            .auth_token_record(token_id)
                            .ok_or(SpendApprovalError::UsedSpendAuthTokenMissing)
                    },
                )
                .expect("payment should settle")
        }
    }

    fn allow_policy(owner_user_id: UserId) -> Policy {
        Policy {
            id: "test-allow-policy".to_string(),
            version: "v1".to_string(),
            owner_user_id,
            default_effect: Effect::NeedsApproval,
            rules: vec![Rule {
                id: "allow_small_spend".to_string(),
                effect: Effect::Allow,
                when: Condition::Lte {
                    field: Field::Amount,
                    value: PolicyValue::MoneyCents(1_000),
                },
                reason: "amount is within the test limit".to_string(),
            }],
        }
    }

    #[test]
    fn authorizes_and_persists_token_and_hold_without_http() {
        let mut harness = ServiceHarness::new(1_000);

        let authorization = match harness.authorize(500, "Acme Cafe") {
            SpendAuthorizationOutcome::Approved(authorization) => authorization,
            SpendAuthorizationOutcome::Rejected(rejection) => {
                panic!("expected approval, got {:?}", rejection.reasons)
            }
        };

        assert_eq!(authorization.amount_cents, 500);
        assert_eq!(
            authorization.budget_reservation.balance.frozen_amount_cents,
            500
        );
        assert_eq!(
            harness
                .governance
                .load_spend_auth_tokens()
                .expect("tokens should load")
                .len(),
            1
        );
        assert_eq!(
            harness
                .governance
                .load_budget_holds()
                .expect("holds should load")
                .len(),
            1
        );
    }

    #[test]
    fn payment_success_marks_token_used_and_settles_hold_without_http() {
        let mut harness = ServiceHarness::new(1_000);
        let authorization = match harness.authorize(500, "Acme Cafe") {
            SpendAuthorizationOutcome::Approved(authorization) => authorization,
            SpendAuthorizationOutcome::Rejected(rejection) => {
                panic!("expected approval, got {:?}", rejection.reasons)
            }
        };

        let settlement = harness.submit_payment(&authorization);

        assert_eq!(settlement.payment.status, PaymentStatus::Succeeded);
        assert!(matches!(
            settlement.budget_update,
            BudgetHoldUpdate::Settled(_)
        ));
        let used_token = harness
            .governance
            .load_spend_auth_tokens()
            .expect("tokens should load")
            .pop()
            .expect("token should persist");
        assert!(used_token.used_at.is_some());
        let holds = harness
            .governance
            .load_budget_holds()
            .expect("holds should load");
        assert!(holds
            .iter()
            .all(|hold| matches!(hold.status, crate::budget::BudgetHoldStatus::Settled)));
        assert_eq!(
            harness
                .payment_attempts
                .list_payment_attempts()
                .expect("attempts should list")
                .len(),
            1
        );
    }

    #[test]
    fn payment_failure_releases_reserved_hold_without_http() {
        let mut harness = ServiceHarness::new(1_000);
        let authorization = match harness.authorize(500, "fail") {
            SpendAuthorizationOutcome::Approved(authorization) => authorization,
            SpendAuthorizationOutcome::Rejected(rejection) => {
                panic!("expected approval, got {:?}", rejection.reasons)
            }
        };

        let settlement = harness.submit_payment(&authorization);

        assert_eq!(settlement.payment.status, PaymentStatus::Failed);
        assert!(matches!(
            settlement.budget_update,
            BudgetHoldUpdate::Released(_)
        ));
        let holds = harness
            .governance
            .load_budget_holds()
            .expect("holds should load");
        assert!(holds
            .iter()
            .all(|hold| matches!(hold.status, crate::budget::BudgetHoldStatus::Released)));
    }

    #[test]
    fn payment_failure_can_keep_hold_frozen_for_retry_without_http() {
        let mut harness = ServiceHarness::new(1_000);
        let authorization = match harness.authorize(500, "fail") {
            SpendAuthorizationOutcome::Approved(authorization) => authorization,
            SpendAuthorizationOutcome::Rejected(rejection) => {
                panic!("expected approval, got {:?}", rejection.reasons)
            }
        };

        let settlement = harness.submit_payment_with_hold_policy(
            &authorization,
            FailedPaymentHoldPolicy::KeepFrozenForRetry,
        );

        assert_eq!(settlement.payment.status, PaymentStatus::Failed);
        assert!(matches!(
            settlement.budget_update,
            BudgetHoldUpdate::Frozen(_)
        ));
        let holds = harness
            .governance
            .load_budget_holds()
            .expect("holds should load");
        assert!(holds
            .iter()
            .all(|hold| matches!(hold.status, crate::budget::BudgetHoldStatus::Frozen)));
        let token = harness
            .governance
            .load_spend_auth_tokens()
            .expect("tokens should load")
            .pop()
            .expect("token should persist");
        assert!(token.used_at.is_none());
    }

    #[test]
    fn insufficient_budget_returns_structured_rejection_without_holds() {
        let mut harness = ServiceHarness::new(400);

        let rejection = match harness.authorize(500, "Acme Cafe") {
            SpendAuthorizationOutcome::Approved(_) => panic!("expected budget rejection"),
            SpendAuthorizationOutcome::Rejected(rejection) => rejection,
        };

        assert_eq!(rejection.decision, Effect::Deny);
        assert_eq!(
            rejection.reasons,
            vec!["budget does not have enough remaining balance".to_string()]
        );
        assert!(harness
            .governance
            .load_budget_holds()
            .expect("holds should load")
            .is_empty());
    }
}
