use std::collections::HashMap;

use chrono::Utc;
use hubu_common::ids::{PaymentId, SpendAuthTokenId, SpendDecisionId};
use hubu_common::models::UserContext;
use serde_json::json;

use crate::policy::engine;
use crate::policy::model::{Effect, Policy};
use crate::spend::error::SpendError;
use crate::spend::model::{
    IssuedSpendAuthToken, SpendAuthTokenRecord, SpendDecisionRecord, SpendEvaluationResponse,
    SpendPaymentValidationRequest, SpendRequest, ValidatedSpendAuthorization,
};
use crate::telemetry::log_event;

const DEFAULT_SPEND_AUTH_TOKEN_TTL: chrono::Duration = chrono::Duration::minutes(5);

/// Spend manager owns in-memory state for spend decisions and issued auth tokens.
pub struct SpendManager {
    decisions: HashMap<SpendDecisionId, SpendDecisionRecord>,
    tokens: HashMap<SpendAuthTokenId, SpendAuthTokenRecord>,
    token_ttl: chrono::Duration,
}

impl SpendManager {
    pub fn new() -> Self {
        Self {
            decisions: HashMap::new(),
            tokens: HashMap::new(),
            token_ttl: DEFAULT_SPEND_AUTH_TOKEN_TTL,
        }
    }

    pub fn from_records(
        decisions: Vec<SpendDecisionRecord>,
        tokens: Vec<SpendAuthTokenRecord>,
    ) -> Self {
        Self {
            decisions: decisions
                .into_iter()
                .map(|decision| (decision.id.clone(), decision))
                .collect(),
            tokens: tokens
                .into_iter()
                .map(|token| (token.id.clone(), token))
                .collect(),
            token_ttl: DEFAULT_SPEND_AUTH_TOKEN_TTL,
        }
    }

    pub fn decision_record(&self, decision_id: &SpendDecisionId) -> Option<SpendDecisionRecord> {
        self.decisions.get(decision_id).cloned()
    }

    pub fn auth_token_record(&self, token_id: &SpendAuthTokenId) -> Option<SpendAuthTokenRecord> {
        self.tokens.get(token_id).cloned()
    }

    pub fn evaluate_spend(
        &mut self,
        user: &UserContext,
        request: SpendRequest,
        policy: &Policy,
    ) -> Result<SpendEvaluationResponse, SpendError> {
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

        let evaluation = engine::evaluate_policy(&request, policy)?;
        let decision_id = SpendDecisionId::new();
        let decision_record = SpendDecisionRecord {
            id: decision_id.clone(),
            owner_user_id: user.user_id.clone(),
            request: request.clone(),
            evaluation: evaluation.clone(),
            created_at: Utc::now(),
        };
        self.decisions.insert(decision_id.clone(), decision_record);

        let auth_token = if evaluation.decision == Effect::Allow {
            let spend_auth_id = SpendAuthTokenId::new();
            let expires_at = Utc::now() + self.token_ttl;
            let spend_auth_record = SpendAuthTokenRecord {
                id: spend_auth_id.clone(),
                owner_user_id: user.user_id.clone(),
                spend_decision_id: decision_id.clone(),
                expires_at,
                used_at: None,
                used_by_payment_id: None,
                revoked_at: None,
            };
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
                "decision_id": decision_id.to_string(),
                "decision": effect_name(evaluation.decision),
                "owner_user_id": user.user_id.to_string(),
                "agent_id": request.agent_id.to_string(),
                "amount_cents": request.amount_cents,
                "currency": request.currency.to_string(),
                "merchant": request.merchant,
                "task_id": request.task_id,
                "policy_id": evaluation.policy_id,
                "policy_version": evaluation.policy_version,
                "matched_rule_count": evaluation.rule_results.iter().filter(|result| result.matched).count(),
                "auth_token_issued": auth_token.is_some(),
                "auth_token_expires_at": auth_token.as_ref().map(|token| token.expires_at.to_rfc3339()),
            }),
        );
        Ok(SpendEvaluationResponse {
            decision_id,
            evaluation,
            auth_token,
        })
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

    pub fn mark_auth_token_used(
        &mut self,
        token_id: &SpendAuthTokenId,
        payment_id: PaymentId,
    ) -> Result<(), SpendError> {
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
    payment.agent_id == spend.agent_id
        && payment.amount_cents == spend.amount_cents
        && payment.currency == spend.currency
        && payment.merchant == spend.merchant
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
    use hubu_common::ids::{AgentId, PaymentId, UserId};

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
            merchant: Some("Acme Cafe".to_string()),
            category: Some("meals".to_string()),
            task_id: Some("task_123".to_string()),
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
            .evaluate_spend(&user_context(), request.clone(), &policy)
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
    fn validates_auth_token_when_payment_matches_authorized_spend() {
        let mut manager = SpendManager::new();
        let request = spend_request(2_500);
        let policy = policy(
            Effect::NeedsApproval,
            vec![amount_rule("allow_small_spend", Effect::Allow, 5_000)],
        );
        let evaluation = manager
            .evaluate_spend(&user_context(), request.clone(), &policy)
            .expect("spend evaluation should succeed");
        let token = evaluation
            .auth_token
            .expect("allow decision should issue auth token");

        let validation = manager
            .validate_auth_token_for_payment(&SpendPaymentValidationRequest {
                spend_auth_token_id: token.id.clone(),
                owner_user_id: request.owner_user_id,
                agent_id: request.agent_id,
                amount_cents: request.amount_cents,
                currency: request.currency,
                merchant: request.merchant,
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
            .evaluate_spend(&user_context(), request.clone(), &policy)
            .expect("spend evaluation should succeed");
        let token = evaluation
            .auth_token
            .expect("allow decision should issue auth token");

        let error = manager
            .validate_auth_token_for_payment(&SpendPaymentValidationRequest {
                spend_auth_token_id: token.id,
                owner_user_id: request.owner_user_id,
                agent_id: request.agent_id,
                amount_cents: request.amount_cents + 1,
                currency: request.currency,
                merchant: request.merchant,
                task_id: request.task_id,
            })
            .expect_err("mismatched amount should fail validation");

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
            .evaluate_spend(&user_context(), request.clone(), &policy)
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
                amount_cents: request.amount_cents,
                currency: request.currency,
                merchant: request.merchant,
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
            .evaluate_spend(&user_context(), request, &policy)
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
            .evaluate_spend(&user_context(), request, &policy)
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
            .evaluate_spend(&user_context(), request, &policy)
            .expect_err("wrong user context should fail");

        assert!(matches!(error, SpendError::UserScopeMismatch));
        assert_eq!(manager.decisions.len(), 0);
    }
}
