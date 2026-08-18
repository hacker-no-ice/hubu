use std::collections::HashMap;

use chrono::{Duration, Utc};
use hubu_common::ids::{
    AgentId, PaymentId, SpendAuthTokenId, SpendDecisionId, SpendExecutorClaimId,
};
use hubu_common::models::UserContext;
use serde_json::json;

use crate::policy::engine;
use crate::policy::model::{Effect, Policy};
use crate::spend::error::SpendError;
use crate::spend::model::{
    IssuedSpendAuthToken, SpendAuthTokenRecord, SpendDecisionRecord, SpendEvaluationResponse,
    SpendExecutorClaimRecord, SpendExecutorClaimRequest, SpendExecutorClaimStatus,
    SpendExecutorClaimValidationRequest, SpendPaymentValidationRequest, SpendRequest,
    SpendTimingConfig, ValidatedSpendAuthorization,
};
use crate::telemetry::log_event;

/// Spend manager owns in-memory state for spend decisions and issued auth tokens.
pub struct SpendManager {
    decisions: HashMap<SpendDecisionId, SpendDecisionRecord>,
    decision_ids_by_operation: HashMap<(AgentId, String), Vec<SpendDecisionId>>,
    tokens: HashMap<SpendAuthTokenId, SpendAuthTokenRecord>,
    token_id_by_decision: HashMap<SpendDecisionId, SpendAuthTokenId>,
    executor_claims: HashMap<SpendExecutorClaimId, SpendExecutorClaimRecord>,
    claim_id_by_token: HashMap<SpendAuthTokenId, SpendExecutorClaimId>,
    claim_id_by_operation: HashMap<(AgentId, String), SpendExecutorClaimId>,
    timing: SpendTimingConfig,
}

impl SpendManager {
    pub fn new() -> Self {
        Self {
            decisions: HashMap::new(),
            decision_ids_by_operation: HashMap::new(),
            tokens: HashMap::new(),
            token_id_by_decision: HashMap::new(),
            executor_claims: HashMap::new(),
            claim_id_by_token: HashMap::new(),
            claim_id_by_operation: HashMap::new(),
            timing: SpendTimingConfig::default(),
        }
    }

    pub fn from_records(
        decisions: Vec<SpendDecisionRecord>,
        tokens: Vec<SpendAuthTokenRecord>,
    ) -> Self {
        Self::from_records_with_claims(decisions, tokens, Vec::new(), SpendTimingConfig::default())
    }

    pub fn from_records_with_claims(
        decisions: Vec<SpendDecisionRecord>,
        tokens: Vec<SpendAuthTokenRecord>,
        executor_claims: Vec<SpendExecutorClaimRecord>,
        timing: SpendTimingConfig,
    ) -> Self {
        let mut decision_ids_by_operation: HashMap<_, Vec<_>> = HashMap::new();
        for decision in &decisions {
            decision_ids_by_operation
                .entry((
                    decision.request.agent_id.clone(),
                    decision.operation_key.clone(),
                ))
                .or_default()
                .push(decision.id.clone());
        }
        let token_id_by_decision = tokens
            .iter()
            .map(|token| (token.spend_decision_id.clone(), token.id.clone()))
            .collect();
        let claim_id_by_token = executor_claims
            .iter()
            .map(|claim| (claim.spend_auth_token_id.clone(), claim.id.clone()))
            .collect();
        let claim_id_by_operation = executor_claims
            .iter()
            .map(|claim| {
                (
                    (claim.agent_id.clone(), claim.operation_key.clone()),
                    claim.id.clone(),
                )
            })
            .collect();
        Self {
            decisions: decisions
                .into_iter()
                .map(|decision| (decision.id.clone(), decision))
                .collect(),
            decision_ids_by_operation,
            tokens: tokens
                .into_iter()
                .map(|token| (token.id.clone(), token))
                .collect(),
            token_id_by_decision,
            executor_claims: executor_claims
                .into_iter()
                .map(|claim| (claim.id.clone(), claim))
                .collect(),
            claim_id_by_token,
            claim_id_by_operation,
            timing,
        }
    }

    pub fn decision_record(&self, decision_id: &SpendDecisionId) -> Option<SpendDecisionRecord> {
        self.decisions.get(decision_id).cloned()
    }

    pub fn has_decision_for_scope(
        &self,
        agent_id: &AgentId,
        operation_key: &str,
        request: &SpendRequest,
    ) -> bool {
        self.decision_ids_by_operation
            .get(&(agent_id.clone(), operation_key.to_string()))
            .into_iter()
            .flatten()
            .any(|decision_id| {
                self.decisions
                    .get(decision_id)
                    .is_some_and(|decision| decision.request.replay_equivalent(request))
            })
    }

    pub fn workload_profile_for_operation(
        &self,
        agent_id: &AgentId,
        operation_key: &str,
    ) -> Option<String> {
        self.decision_ids_by_operation
            .get(&(agent_id.clone(), operation_key.to_string()))
            .and_then(|decision_ids| decision_ids.last())
            .and_then(|decision_id| self.decisions.get(decision_id))
            .map(|decision| decision.request.workload_profile.clone())
    }

    pub fn auth_token_record(&self, token_id: &SpendAuthTokenId) -> Option<SpendAuthTokenRecord> {
        self.tokens.get(token_id).cloned()
    }

    pub fn executor_claim_record(
        &self,
        claim_id: &SpendExecutorClaimId,
    ) -> Option<SpendExecutorClaimRecord> {
        self.executor_claims.get(claim_id).cloned()
    }

    pub fn executor_claim_for_operation(
        &self,
        agent_id: &AgentId,
        operation_key: &str,
    ) -> Option<SpendExecutorClaimRecord> {
        self.claim_id_by_operation
            .get(&(agent_id.clone(), operation_key.to_string()))
            .and_then(|claim_id| self.executor_claim_record(claim_id))
    }

    pub fn executor_claim_records_for_owner(
        &self,
        owner_user_id: &hubu_common::ids::UserId,
    ) -> Vec<SpendExecutorClaimRecord> {
        let mut claims = self
            .executor_claims
            .values()
            .filter(|claim| &claim.owner_user_id == owner_user_id)
            .cloned()
            .collect::<Vec<_>>();
        claims.sort_by(|left, right| {
            left.expires_at
                .cmp(&right.expires_at)
                .then_with(|| left.id.to_string().cmp(&right.id.to_string()))
        });
        claims
    }

    pub fn apply_persisted_executor_finalization(
        &mut self,
        claim: SpendExecutorClaimRecord,
        token: SpendAuthTokenRecord,
    ) {
        self.claim_id_by_token
            .insert(token.id.clone(), claim.id.clone());
        self.claim_id_by_operation.insert(
            (claim.agent_id.clone(), claim.operation_key.clone()),
            claim.id.clone(),
        );
        self.tokens.insert(token.id.clone(), token);
        self.executor_claims.insert(claim.id.clone(), claim);
    }

    pub fn evaluate_spend(
        &mut self,
        user: &UserContext,
        operation_key: &str,
        request: SpendRequest,
        policy: &Policy,
    ) -> Result<SpendEvaluationResponse, SpendError> {
        self.evaluate_spend_with_revision(user, operation_key, request, policy, None)
    }

    pub fn evaluate_spend_at_revision(
        &mut self,
        user: &UserContext,
        operation_key: &str,
        request: SpendRequest,
        policy: &Policy,
        revision: u64,
        actor: &str,
    ) -> Result<SpendEvaluationResponse, SpendError> {
        self.evaluate_spend_with_revision(
            user,
            operation_key,
            request,
            policy,
            Some((revision, actor)),
        )
    }

    fn evaluate_spend_with_revision(
        &mut self,
        user: &UserContext,
        operation_key: &str,
        request: SpendRequest,
        policy: &Policy,
        revision_and_actor: Option<(u64, &str)>,
    ) -> Result<SpendEvaluationResponse, SpendError> {
        let operation_key = operation_key.trim();
        if operation_key.is_empty() {
            return Err(SpendError::EmptyOperationKey);
        }
        if request.owner_user_id != user.user_id || policy.owner_user_id != user.user_id {
            log_event(
                "warn",
                "spend_evaluation_rejected",
                json!({
                    "reason": "user_scope_mismatch",
                    "user_id": user.user_id.to_string(),
                    "request_owner_user_id": request.owner_user_id.to_string(),
                    "policy_owner_user_id": policy.owner_user_id.to_string(),
                    "agent_id": request.agent_id.to_string(),
                    "amount_cents": request.amount_cents,
                }),
            );
            return Err(SpendError::UserScopeMismatch);
        }

        if let Some(decision_id) = self
            .decision_ids_by_operation
            .get(&(request.agent_id.clone(), operation_key.to_string()))
            .into_iter()
            .flatten()
            .find(|decision_id| {
                self.decisions
                    .get(*decision_id)
                    .is_some_and(|decision| decision.request.replay_equivalent(&request))
            })
        {
            let decision = self
                .decisions
                .get(decision_id)
                .ok_or(SpendError::MissingSpendDecision)?;
            if !decision.request.replay_equivalent(&request) {
                return Err(SpendError::OperationKeyConflict);
            }
            let auth_token = self
                .token_id_by_decision
                .get(decision_id)
                .and_then(|token_id| self.tokens.get(token_id))
                .map(|token| IssuedSpendAuthToken {
                    id: token.id.clone(),
                    expires_at: token.expires_at,
                });
            return Ok(SpendEvaluationResponse {
                operation_key: decision.operation_key.clone(),
                decision_id: decision.id.clone(),
                evaluation: decision.evaluation.clone(),
                auth_token,
                idempotent_replay: true,
                revision: decision.revision,
                retry_guidance: crate::spend::SpendRetryGuidance {
                    action: crate::spend::SpendRetryAction::ReplayExactly,
                    operation_key: decision.operation_key.clone(),
                    message: "replay this exact immutable scope to recover the original result"
                        .to_string(),
                },
                attempt_history: Vec::new(),
            });
        }

        let timing = self
            .timing
            .profile(&request.workload_profile)
            .ok_or_else(|| SpendError::UnknownWorkloadProfile(request.workload_profile.clone()))?;
        let authorization_ttl = Duration::seconds(timing.authorization_ttl_seconds);
        let claim_ttl_seconds = timing.claim_ttl_seconds;
        let evaluation = engine::evaluate_policy(&request, policy)?;
        let decision_id = SpendDecisionId::new();
        let decision_record = SpendDecisionRecord {
            id: decision_id.clone(),
            owner_user_id: user.user_id.clone(),
            operation_key: operation_key.to_string(),
            revision: revision_and_actor.map_or_else(
                || {
                    self.decision_ids_by_operation
                        .get(&(request.agent_id.clone(), operation_key.to_string()))
                        .map_or(1, |ids| ids.len() as u64 + 1)
                },
                |(revision, _)| revision,
            ),
            actor: revision_and_actor
                .map_or_else(|| user.user_id.to_string(), |(_, actor)| actor.to_string()),
            request: request.clone(),
            evaluation: evaluation.clone(),
            created_at: Utc::now(),
        };
        self.decision_ids_by_operation
            .entry((request.agent_id.clone(), operation_key.to_string()))
            .or_default()
            .push(decision_id.clone());
        let revision = decision_record.revision;
        self.decisions.insert(decision_id.clone(), decision_record);

        let auth_token = if evaluation.decision == Effect::Allow {
            let spend_auth_id = SpendAuthTokenId::new();
            let expires_at = Utc::now() + authorization_ttl;
            let spend_auth_record = SpendAuthTokenRecord {
                id: spend_auth_id.clone(),
                owner_user_id: user.user_id.clone(),
                spend_decision_id: decision_id.clone(),
                expires_at,
                claim_ttl_seconds,
                used_at: None,
                used_by_payment_id: None,
                revoked_at: None,
            };
            self.token_id_by_decision
                .insert(decision_id.clone(), spend_auth_id.clone());
            self.tokens.insert(spend_auth_id.clone(), spend_auth_record);

            Some(IssuedSpendAuthToken {
                id: spend_auth_id,
                expires_at,
            })
        } else {
            None
        };

        log_event(
            "info",
            "spend_evaluated",
            json!({
                "operation_key": operation_key,
                "decision_id": decision_id.to_string(),
                "decision": effect_name(evaluation.decision),
                "owner_user_id": user.user_id.to_string(),
                "agent_id": request.agent_id.to_string(),
                "amount_cents": request.amount_cents,
                "currency": request.currency.to_string(),
                "merchant": request.merchant,
                "execution_scope": request.execution_scope,
                "task_id": request.task_id,
                "policy_id": evaluation.policy_id,
                "policy_version": evaluation.policy_version,
                "matched_rule_count": evaluation.rule_results.iter().filter(|result| result.matched).count(),
                "auth_token_issued": auth_token.is_some(),
                "auth_token_expires_at": auth_token.as_ref().map(|token| token.expires_at.to_rfc3339()),
            }),
        );
        Ok(SpendEvaluationResponse {
            operation_key: operation_key.to_string(),
            decision_id,
            evaluation,
            auth_token,
            idempotent_replay: false,
            revision,
            retry_guidance: crate::spend::SpendRetryGuidance {
                action: crate::spend::SpendRetryAction::ReplayExactly,
                operation_key: operation_key.to_string(),
                message: "replay this exact immutable scope to recover the original result"
                    .to_string(),
            },
            attempt_history: Vec::new(),
        })
    }

    pub fn discard_auth_token_for_decision(&mut self, decision_id: &SpendDecisionId) {
        if let Some(token_id) = self.token_id_by_decision.remove(decision_id) {
            self.tokens.remove(&token_id);
        }
    }

    pub fn validate_auth_token_for_payment(
        &self,
        request: &SpendPaymentValidationRequest,
    ) -> Result<ValidatedSpendAuthorization, SpendError> {
        let token = self
            .tokens
            .get(&request.spend_auth_token_id)
            .ok_or_else(|| {
                log_auth_validation_rejected(request, "unknown_spend_auth_token");
                SpendError::UnknownSpendAuthToken
            })?;

        if token.revoked_at.is_some() {
            log_auth_validation_rejected(request, "revoked_spend_auth_token");
            return Err(SpendError::RevokedSpendAuthToken);
        }

        if token.used_at.is_some() {
            log_auth_validation_rejected(request, "used_spend_auth_token");
            return Err(SpendError::UsedSpendAuthToken);
        }

        if self
            .claim_id_by_token
            .contains_key(&request.spend_auth_token_id)
        {
            log_auth_validation_rejected(request, "spend_auth_token_already_claimed");
            return Err(SpendError::SpendAuthTokenAlreadyClaimed);
        }

        if token.expires_at <= Utc::now() {
            log_auth_validation_rejected(request, "expired_spend_auth_token");
            return Err(SpendError::ExpiredSpendAuthToken);
        }

        let decision = self
            .decisions
            .get(&token.spend_decision_id)
            .ok_or_else(|| {
                log_auth_validation_rejected(request, "missing_spend_decision");
                SpendError::MissingSpendDecision
            })?;

        if decision.evaluation.decision != Effect::Allow {
            log_auth_validation_rejected(request, "spend_decision_not_allowed");
            return Err(SpendError::SpendDecisionNotAllowed);
        }

        if request.owner_user_id != decision.owner_user_id {
            log_auth_validation_rejected(request, "user_scope_mismatch");
            return Err(SpendError::UserScopeMismatch);
        }

        if !payment_matches_authorized_spend(request, &decision.request) {
            log_auth_validation_rejected(request, "payment_request_mismatch");
            return Err(SpendError::PaymentRequestMismatch);
        }

        log_event(
            "info",
            "spend_auth_token_validated",
            json!({
                "spend_auth_token_id": token.id.to_string(),
                "spend_decision_id": token.spend_decision_id.to_string(),
                "owner_user_id": token.owner_user_id.to_string(),
                "agent_id": request.agent_id.to_string(),
                "amount_cents": request.amount_cents,
                "currency": request.currency.to_string(),
            }),
        );
        Ok(ValidatedSpendAuthorization {
            spend_auth_token_id: token.id.clone(),
            owner_user_id: token.owner_user_id.clone(),
            spend_decision_id: token.spend_decision_id.clone(),
            expires_at: token.expires_at,
        })
    }

    pub fn claim_auth_token(
        &mut self,
        request: SpendExecutorClaimRequest,
    ) -> Result<(SpendExecutorClaimRecord, ValidatedSpendAuthorization), SpendError> {
        let (validation, existing_claim) = self.validate_auth_token_for_executor_claim(
            &request.authorization,
            &request.operation_key,
        )?;
        if let Some(claim) = existing_claim {
            return Ok((claim, validation));
        }

        let decision = self
            .decisions
            .get(&validation.spend_decision_id)
            .ok_or(SpendError::MissingSpendDecision)?;
        let workload_profile = decision.request.workload_profile.clone();
        let claim_ttl_seconds = self
            .tokens
            .get(&request.authorization.spend_auth_token_id)
            .ok_or(SpendError::UnknownSpendAuthToken)?
            .claim_ttl_seconds;
        let claimed_at = Utc::now();
        let claim = SpendExecutorClaimRecord {
            id: SpendExecutorClaimId::new(),
            spend_auth_token_id: request.authorization.spend_auth_token_id.clone(),
            owner_user_id: request.authorization.owner_user_id.clone(),
            agent_id: request.authorization.agent_id.clone(),
            operation_key: request.operation_key.clone(),
            workload_profile,
            status: SpendExecutorClaimStatus::Claimed,
            claimed_at,
            expires_at: claimed_at + Duration::seconds(claim_ttl_seconds),
            finalized_at: None,
            settlement_id: None,
            provider_reference: None,
            reconciliation_evidence: None,
            reconciled_at: None,
            reconciled_by_user_id: None,
        };
        self.claim_id_by_token
            .insert(claim.spend_auth_token_id.clone(), claim.id.clone());
        self.claim_id_by_operation.insert(
            (claim.agent_id.clone(), claim.operation_key.clone()),
            claim.id.clone(),
        );
        self.executor_claims.insert(claim.id.clone(), claim.clone());

        Ok((claim, validation))
    }

    pub fn validate_auth_token_for_executor_claim(
        &self,
        authorization: &SpendPaymentValidationRequest,
        operation_key: &str,
    ) -> Result<
        (
            ValidatedSpendAuthorization,
            Option<SpendExecutorClaimRecord>,
        ),
        SpendError,
    > {
        if let Some(claim) =
            self.executor_claim_for_operation(&authorization.agent_id, operation_key)
        {
            if claim.spend_auth_token_id != authorization.spend_auth_token_id {
                return Err(SpendError::OperationKeyConflict);
            }
            let validation = self.validate_authorization_scope(authorization, true)?;
            return Ok((validation, Some(claim)));
        }

        if let Some(claim_id) = self
            .claim_id_by_token
            .get(&authorization.spend_auth_token_id)
        {
            let claim = self
                .executor_claims
                .get(claim_id)
                .cloned()
                .ok_or(SpendError::UnknownExecutorClaim)?;
            if claim.operation_key != operation_key {
                return Err(SpendError::SpendAuthTokenAlreadyClaimed);
            }
            if matches!(claim.status, SpendExecutorClaimStatus::Claimed)
                && claim.expires_at <= Utc::now()
            {
                return Err(SpendError::ExpiredExecutorClaim);
            }
            let validation = self.validate_authorization_scope(authorization, true)?;
            return Ok((validation, Some(claim)));
        }

        let validation = self.validate_auth_token_for_payment(authorization)?;
        let decision = self
            .decisions
            .get(&validation.spend_decision_id)
            .ok_or(SpendError::MissingSpendDecision)?;
        if decision.operation_key != operation_key {
            return Err(SpendError::OperationKeyConflict);
        }

        Ok((validation, None))
    }

    pub fn validate_executor_claim(
        &self,
        request: &SpendExecutorClaimValidationRequest,
    ) -> Result<SpendExecutorClaimRecord, SpendError> {
        let claim = self
            .executor_claim_for_operation(&request.agent_id, &request.operation_key)
            .ok_or(SpendError::UnknownExecutorClaim)?;
        if claim.owner_user_id != request.owner_user_id {
            return Err(SpendError::UserScopeMismatch);
        }
        if claim.agent_id != request.agent_id {
            return Err(SpendError::ExecutorClaimOperationMismatch);
        }
        if claim.operation_key != request.operation_key {
            return Err(SpendError::ExecutorClaimOperationMismatch);
        }
        if !matches!(claim.status, SpendExecutorClaimStatus::Claimed) {
            return Err(SpendError::FinalizedExecutorClaim);
        }
        if claim.expires_at <= Utc::now() {
            return Err(SpendError::ExpiredExecutorClaim);
        }
        let token = self
            .tokens
            .get(&claim.spend_auth_token_id)
            .ok_or(SpendError::UnknownSpendAuthToken)?;
        if token.revoked_at.is_some() {
            return Err(SpendError::RevokedSpendAuthToken);
        }
        if token.used_at.is_some() {
            return Err(SpendError::UsedSpendAuthToken);
        }
        Ok(claim)
    }

    fn validate_authorization_scope(
        &self,
        request: &SpendPaymentValidationRequest,
        allow_finalized: bool,
    ) -> Result<ValidatedSpendAuthorization, SpendError> {
        let token = self
            .tokens
            .get(&request.spend_auth_token_id)
            .ok_or(SpendError::UnknownSpendAuthToken)?;
        if !allow_finalized && token.revoked_at.is_some() {
            return Err(SpendError::RevokedSpendAuthToken);
        }
        if !allow_finalized && token.used_at.is_some() {
            return Err(SpendError::UsedSpendAuthToken);
        }
        let decision = self
            .decisions
            .get(&token.spend_decision_id)
            .ok_or(SpendError::MissingSpendDecision)?;
        if decision.evaluation.decision != Effect::Allow {
            return Err(SpendError::SpendDecisionNotAllowed);
        }
        if request.owner_user_id != decision.owner_user_id {
            return Err(SpendError::UserScopeMismatch);
        }
        if !payment_matches_authorized_spend(request, &decision.request) {
            return Err(SpendError::PaymentRequestMismatch);
        }
        Ok(ValidatedSpendAuthorization {
            spend_auth_token_id: token.id.clone(),
            owner_user_id: token.owner_user_id.clone(),
            spend_decision_id: token.spend_decision_id.clone(),
            expires_at: token.expires_at,
        })
    }

    pub fn mark_auth_token_used(
        &mut self,
        token_id: &SpendAuthTokenId,
        payment_id: PaymentId,
    ) -> Result<(), SpendError> {
        if self.claim_id_by_token.contains_key(token_id) {
            return Err(SpendError::SpendAuthTokenAlreadyClaimed);
        }
        let token = self.tokens.get_mut(token_id).ok_or_else(|| {
            log_event(
                "warn",
                "spend_auth_token_mark_used_rejected",
                json!({
                    "reason": "unknown_spend_auth_token",
                    "spend_auth_token_id": token_id.to_string(),
                    "payment_id": payment_id.to_string(),
                }),
            );
            SpendError::UnknownSpendAuthToken
        })?;

        if token.used_at.is_some() {
            log_event(
                "warn",
                "spend_auth_token_mark_used_rejected",
                json!({
                    "reason": "used_spend_auth_token",
                    "spend_auth_token_id": token_id.to_string(),
                    "payment_id": payment_id.to_string(),
                }),
            );
            return Err(SpendError::UsedSpendAuthToken);
        }

        token.used_at = Some(Utc::now());
        token.used_by_payment_id = Some(payment_id);

        log_event(
            "info",
            "spend_auth_token_marked_used",
            json!({
                "spend_auth_token_id": token.id.to_string(),
                "spend_decision_id": token.spend_decision_id.to_string(),
                "owner_user_id": token.owner_user_id.to_string(),
                "payment_id": token.used_by_payment_id.as_ref().map(ToString::to_string),
            }),
        );
        Ok(())
    }
}

fn payment_matches_authorized_spend(
    payment: &SpendPaymentValidationRequest,
    spend: &SpendRequest,
) -> bool {
    let execution_scope_matches = payment.execution_scope == spend.execution_scope
        || (spend.execution_scope.is_none()
            && spend.merchant.is_some()
            && payment.merchant == spend.merchant);
    payment.agent_id == spend.agent_id
        && payment.agent_account_id == spend.agent_account_id
        && payment.amount_cents == spend.amount_cents
        && payment.currency == spend.currency
        && payment.merchant == spend.merchant
        && execution_scope_matches
        && payment.task_id == spend.task_id
}

fn log_auth_validation_rejected(request: &SpendPaymentValidationRequest, reason: &str) {
    log_event(
        "warn",
        "spend_auth_token_validation_rejected",
        json!({
            "reason": reason,
            "spend_auth_token_id": request.spend_auth_token_id.to_string(),
            "owner_user_id": request.owner_user_id.to_string(),
            "agent_id": request.agent_id.to_string(),
            "amount_cents": request.amount_cents,
            "currency": request.currency.to_string(),
            "merchant": request.merchant,
            "execution_scope": request.execution_scope,
            "task_id": request.task_id,
        }),
    );
}

fn effect_name(effect: Effect) -> &'static str {
    match effect {
        Effect::Allow => "allow",
        Effect::Deny => "deny",
        Effect::NeedsApproval => "needs_approval",
    }
}

impl Default for SpendManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use hubu_common::ids::{AgentAccountId, AgentId, PaymentId, UserId};

    use super::*;
    use crate::policy::condition::{Condition, Field, PolicyValue};
    use hubu_common::money::Currency;

    use crate::policy::model::Rule;

    fn spend_request(amount_cents: i64) -> SpendRequest {
        SpendRequest {
            amount_cents,
            currency: Currency::Usd,
            owner_user_id: test_user_id(),
            agent_id: AgentId::new(),
            agent_account_id: AgentAccountId::new(),
            merchant: Some("Acme Cafe".to_string()),
            execution_scope: None,
            category: Some("meals".to_string()),
            task_id: Some("task_123".to_string()),
            reason: "Team lunch".to_string(),
            workload_profile: "default".to_string(),
        }
    }

    fn policy(default_effect: Effect, rules: Vec<Rule>) -> Policy {
        Policy {
            id: "base_spending_policy".to_string(),
            version: "2026-05-22.1".to_string(),
            owner_user_id: test_user_id(),
            rules,
            default_effect,
        }
    }

    fn test_user_id() -> UserId {
        "00000000-0000-4000-8000-000000000123".parse().unwrap()
    }

    fn user_context() -> UserContext {
        UserContext::new(test_user_id())
    }

    fn amount_rule(id: &str, effect: Effect, limit_cents: i64) -> Rule {
        Rule {
            id: id.to_string(),
            effect,
            when: Condition::Lte {
                field: Field::Amount,
                value: PolicyValue::MoneyCents(limit_cents),
            },
            reason: format!("{id} matched"),
        }
    }

    #[test]
    fn allow_decision_stores_decision_and_issues_auth_token() {
        let mut manager = SpendManager::new();
        let request = spend_request(2_500);
        let policy = policy(
            Effect::NeedsApproval,
            vec![amount_rule("allow_small_spend", Effect::Allow, 5_000)],
        );
        let before = Utc::now();

        let response = manager
            .evaluate_spend(&user_context(), "test-job", request.clone(), &policy)
            .expect("spend evaluation should succeed");

        assert_eq!(response.evaluation.decision, Effect::Allow);
        assert_eq!(manager.decisions.len(), 1);
        assert_eq!(manager.tokens.len(), 1);

        let decision_record = manager
            .decisions
            .get(&response.decision_id)
            .expect("decision record should be stored");
        assert_eq!(decision_record.id, response.decision_id);
        assert_eq!(decision_record.owner_user_id, test_user_id());
        assert_eq!(decision_record.request.task_id, request.task_id);
        assert_eq!(decision_record.evaluation.decision, Effect::Allow);

        let token = response
            .auth_token
            .expect("allow decision should issue auth token");
        assert!(token.expires_at > before);

        let token_record = manager
            .tokens
            .get(&token.id)
            .expect("auth token record should be stored");
        assert_eq!(token_record.id, token.id);
        assert_eq!(token_record.owner_user_id, test_user_id());
        assert_eq!(token_record.spend_decision_id, response.decision_id);
        assert_eq!(token_record.expires_at, token.expires_at);
        assert_eq!(token_record.used_at, None);
        assert_eq!(token_record.used_by_payment_id, None);
        assert_eq!(token_record.revoked_at, None);
    }

    #[test]
    fn authorization_operation_replays_historical_scope_and_tracks_new_revision() {
        let mut manager = SpendManager::new();
        let request = spend_request(2_500);
        let policy = policy(Effect::Allow, Vec::new());

        let first = manager
            .evaluate_spend(&user_context(), "stable-job-1", request.clone(), &policy)
            .expect("first authorization should evaluate");
        let retry = manager
            .evaluate_spend(&user_context(), " stable-job-1 ", request.clone(), &policy)
            .expect("same operation and scope should replay");

        assert!(!first.idempotent_replay);
        assert!(retry.idempotent_replay);
        assert_eq!(retry.operation_key, "stable-job-1");
        assert_eq!(retry.decision_id, first.decision_id);
        assert_eq!(
            retry.auth_token.as_ref().map(|token| &token.id),
            first.auth_token.as_ref().map(|token| &token.id)
        );
        assert_eq!(manager.decisions.len(), 1);
        assert_eq!(manager.tokens.len(), 1);

        let mut changed = request.clone();
        changed.amount_cents += 1;
        let corrected = manager
            .evaluate_spend(&user_context(), "stable-job-1", changed, &policy)
            .expect("SQLite admission, not the process-local manager, governs corrected scope");
        assert_eq!(corrected.revision, 2);
        assert_eq!(manager.decisions.len(), 2);
        assert_eq!(manager.tokens.len(), 2);

        let historical = manager
            .evaluate_spend(&user_context(), "stable-job-1", request, &policy)
            .expect("the original immutable scope remains replayable");
        assert!(historical.idempotent_replay);
        assert_eq!(historical.decision_id, first.decision_id);
    }

    #[test]
    fn different_agents_can_reuse_the_same_operation_key() {
        let mut manager = SpendManager::new();
        let first_request = spend_request(2_500);
        let mut second_request = first_request.clone();
        second_request.agent_id = AgentId::new();
        second_request.agent_account_id = hubu_common::ids::AgentAccountId::new();
        let policy = policy(Effect::Allow, Vec::new());

        let first = manager
            .evaluate_spend(&user_context(), "agent-local-job-1", first_request, &policy)
            .expect("first agent should authorize its operation");
        let second = manager
            .evaluate_spend(
                &user_context(),
                "agent-local-job-1",
                second_request,
                &policy,
            )
            .expect("second agent should reuse the local operation key");

        assert_ne!(first.decision_id, second.decision_id);
        assert!(!first.idempotent_replay);
        assert!(!second.idempotent_replay);
        assert_eq!(manager.decisions.len(), 2);
        assert_eq!(manager.tokens.len(), 2);
    }

    #[test]
    fn validates_auth_token_when_payment_matches_authorized_spend() {
        let mut manager = SpendManager::new();
        let request = spend_request(2_500);
        let policy = policy(
            Effect::NeedsApproval,
            vec![amount_rule("allow_small_spend", Effect::Allow, 5_000)],
        );
        let evaluation = manager
            .evaluate_spend(&user_context(), "test-job", request.clone(), &policy)
            .expect("spend evaluation should succeed");
        let token = evaluation
            .auth_token
            .expect("allow decision should issue auth token");

        let validation = manager
            .validate_auth_token_for_payment(&SpendPaymentValidationRequest {
                spend_auth_token_id: token.id.clone(),
                owner_user_id: request.owner_user_id,
                agent_id: request.agent_id,
                agent_account_id: request.agent_account_id,
                amount_cents: request.amount_cents,
                currency: request.currency,
                merchant: request.merchant,
                execution_scope: request.execution_scope,
                task_id: request.task_id,
            })
            .expect("matching payment should validate");

        assert_eq!(validation.owner_user_id, test_user_id());
        assert_eq!(validation.spend_auth_token_id, token.id);
        assert_eq!(validation.spend_decision_id, evaluation.decision_id);
    }

    #[test]
    fn rejects_auth_token_when_payment_amount_differs_from_authorized_spend() {
        let mut manager = SpendManager::new();
        let request = spend_request(2_500);
        let policy = policy(
            Effect::NeedsApproval,
            vec![amount_rule("allow_small_spend", Effect::Allow, 5_000)],
        );
        let evaluation = manager
            .evaluate_spend(&user_context(), "test-job", request.clone(), &policy)
            .expect("spend evaluation should succeed");
        let token = evaluation
            .auth_token
            .expect("allow decision should issue auth token");

        let error = manager
            .validate_auth_token_for_payment(&SpendPaymentValidationRequest {
                spend_auth_token_id: token.id,
                owner_user_id: request.owner_user_id,
                agent_id: request.agent_id,
                agent_account_id: request.agent_account_id,
                amount_cents: request.amount_cents + 1,
                currency: request.currency,
                merchant: request.merchant,
                execution_scope: request.execution_scope,
                task_id: request.task_id,
            })
            .expect_err("mismatched amount should fail validation");

        assert!(matches!(error, SpendError::PaymentRequestMismatch));
    }

    #[test]
    fn rejects_auth_token_when_payment_account_differs_from_authorized_spend() {
        let mut manager = SpendManager::new();
        let request = spend_request(2_500);
        let policy = policy(
            Effect::NeedsApproval,
            vec![amount_rule("allow_small_spend", Effect::Allow, 5_000)],
        );
        let evaluation = manager
            .evaluate_spend(&user_context(), "test-job", request.clone(), &policy)
            .expect("spend evaluation should succeed");
        let token = evaluation
            .auth_token
            .expect("allow decision should issue auth token");

        let error = manager
            .validate_auth_token_for_payment(&SpendPaymentValidationRequest {
                spend_auth_token_id: token.id,
                owner_user_id: request.owner_user_id,
                agent_id: request.agent_id,
                agent_account_id: AgentAccountId::new(),
                amount_cents: request.amount_cents,
                currency: request.currency,
                merchant: request.merchant,
                execution_scope: request.execution_scope,
                task_id: request.task_id,
            })
            .expect_err("mismatched account should fail validation");

        assert!(matches!(error, SpendError::PaymentRequestMismatch));
    }

    #[test]
    fn used_auth_token_cannot_validate_again() {
        let mut manager = SpendManager::new();
        let request = spend_request(2_500);
        let policy = policy(
            Effect::NeedsApproval,
            vec![amount_rule("allow_small_spend", Effect::Allow, 5_000)],
        );
        let evaluation = manager
            .evaluate_spend(&user_context(), "test-job", request.clone(), &policy)
            .expect("spend evaluation should succeed");
        let token = evaluation
            .auth_token
            .expect("allow decision should issue auth token");

        manager
            .mark_auth_token_used(&token.id, PaymentId::new())
            .expect("token should be marked used");

        let error = manager
            .validate_auth_token_for_payment(&SpendPaymentValidationRequest {
                spend_auth_token_id: token.id,
                owner_user_id: request.owner_user_id,
                agent_id: request.agent_id,
                agent_account_id: request.agent_account_id,
                amount_cents: request.amount_cents,
                currency: request.currency,
                merchant: request.merchant,
                execution_scope: request.execution_scope,
                task_id: request.task_id,
            })
            .expect_err("used token should fail validation");

        assert!(matches!(error, SpendError::UsedSpendAuthToken));
    }

    #[test]
    fn non_allow_decision_stores_decision_without_issuing_auth_token() {
        let mut manager = SpendManager::new();
        let request = spend_request(8_000);
        let policy = policy(
            Effect::NeedsApproval,
            vec![amount_rule("allow_small_spend", Effect::Allow, 5_000)],
        );

        let response = manager
            .evaluate_spend(&user_context(), "test-job", request, &policy)
            .expect("spend evaluation should succeed");

        assert_eq!(response.evaluation.decision, Effect::NeedsApproval);
        assert!(response.auth_token.is_none());
        assert_eq!(manager.decisions.len(), 1);
        assert_eq!(manager.tokens.len(), 0);
        assert!(manager.decisions.contains_key(&response.decision_id));
    }

    #[test]
    fn invalid_policy_returns_error_without_storing_state() {
        let mut manager = SpendManager::new();
        let request = spend_request(2_500);
        let policy = Policy {
            id: String::new(),
            version: "2026-05-22.1".to_string(),
            owner_user_id: test_user_id(),
            rules: Vec::new(),
            default_effect: Effect::Allow,
        };

        let error = manager
            .evaluate_spend(&user_context(), "test-job", request, &policy)
            .expect_err("invalid policy should fail spend evaluation");

        assert!(matches!(error, SpendError::PolicyValidation { .. }));
        assert_eq!(manager.decisions.len(), 0);
        assert_eq!(manager.tokens.len(), 0);
    }

    #[test]
    fn rejects_spend_evaluation_outside_user_context() {
        let mut manager = SpendManager::new();
        let mut request = spend_request(2_500);
        request.owner_user_id = "00000000-0000-4000-8000-000000000456".parse().unwrap();
        let policy = policy(Effect::Allow, Vec::new());

        let error = manager
            .evaluate_spend(&user_context(), "test-job", request, &policy)
            .expect_err("wrong user context should fail");

        assert!(matches!(error, SpendError::UserScopeMismatch));
        assert_eq!(manager.decisions.len(), 0);
    }

    #[test]
    fn executor_claim_uses_profile_lease_and_survives_authorization_expiry() {
        let timing = SpendTimingConfig {
            default_profile: "image".to_string(),
            profiles: HashMap::from([(
                "image".to_string(),
                crate::spend::SpendTimingProfile {
                    authorization_ttl_seconds: 60,
                    claim_ttl_seconds: 600,
                },
            )]),
        };
        let mut manager =
            SpendManager::from_records_with_claims(Vec::new(), Vec::new(), Vec::new(), timing);
        let mut spend = spend_request(2_500);
        spend.workload_profile = "image".to_string();
        let evaluation = manager
            .evaluate_spend(
                &user_context(),
                "gongbu-image-123",
                spend.clone(),
                &policy(Effect::Allow, Vec::new()),
            )
            .expect("spend should authorize");
        let operation_key = evaluation.operation_key.clone();
        let token = evaluation.auth_token.expect("allow should issue token");
        let authorization = SpendPaymentValidationRequest {
            spend_auth_token_id: token.id.clone(),
            owner_user_id: spend.owner_user_id.clone(),
            agent_id: spend.agent_id.clone(),
            agent_account_id: spend.agent_account_id.clone(),
            amount_cents: spend.amount_cents,
            currency: spend.currency,
            merchant: spend.merchant.clone(),
            execution_scope: spend.execution_scope.clone(),
            task_id: spend.task_id.clone(),
        };
        let (claim, _) = manager
            .claim_auth_token(SpendExecutorClaimRequest {
                authorization,
                operation_key: operation_key.clone(),
            })
            .expect("executor should claim token");

        assert_eq!(claim.workload_profile, "image");
        assert!(claim.expires_at > token.expires_at);
        manager
            .tokens
            .get_mut(&token.id)
            .expect("token should remain stored")
            .expires_at = Utc::now() - Duration::seconds(1);

        manager
            .validate_executor_claim(&SpendExecutorClaimValidationRequest {
                owner_user_id: spend.owner_user_id,
                agent_id: spend.agent_id,
                operation_key,
            })
            .expect("active claim should outlive original authorization");
    }

    #[test]
    fn executor_claim_is_idempotent_for_same_operation_and_exclusive_for_others() {
        let mut manager = SpendManager::new();
        let spend = spend_request(2_500);
        let evaluation = manager
            .evaluate_spend(
                &user_context(),
                "gongbu-job-1",
                spend.clone(),
                &policy(Effect::Allow, Vec::new()),
            )
            .expect("spend should authorize");
        let operation_key = evaluation.operation_key.clone();
        let token = evaluation.auth_token.expect("allow should issue token");
        let authorization = SpendPaymentValidationRequest {
            spend_auth_token_id: token.id,
            owner_user_id: spend.owner_user_id,
            agent_id: spend.agent_id,
            agent_account_id: spend.agent_account_id,
            amount_cents: spend.amount_cents,
            currency: spend.currency,
            merchant: spend.merchant,
            execution_scope: spend.execution_scope,
            task_id: spend.task_id,
        };
        let request = SpendExecutorClaimRequest {
            authorization: authorization.clone(),
            operation_key,
        };
        let (first, _) = manager
            .claim_auth_token(request.clone())
            .expect("first claim should succeed");
        let (retry, _) = manager
            .claim_auth_token(request)
            .expect("same operation should receive the existing claim");
        assert_eq!(first.id, retry.id);

        let error = manager
            .claim_auth_token(SpendExecutorClaimRequest {
                authorization,
                operation_key: "another-operation".to_string(),
            })
            .expect_err("another operation must not claim the token");
        assert!(matches!(error, SpendError::SpendAuthTokenAlreadyClaimed));
    }
}
