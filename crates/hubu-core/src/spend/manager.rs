use std::collections::HashMap;

use chrono::Utc;
use hubu_common::ids::{SpendAuthTokenId, SpendDecisionId};

use crate::policy::engine;
use crate::policy::model::{Effect, Policy};
use crate::spend::error::SpendError;
use crate::spend::model::{
    IssuedSpendAuthToken, SpendAuthTokenRecord, SpendDecisionRecord, SpendEvaluationResponse,
    SpendRequest,
};

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

    pub fn evaluate_spend(
        &mut self,
        request: SpendRequest,
        policy: &Policy,
    ) -> Result<SpendEvaluationResponse, SpendError> {
        let evaluation = engine::evaluate_policy(&request, policy)?;
        let decision_id = SpendDecisionId::new();
        let decision_record = SpendDecisionRecord {
            id: decision_id.clone(),
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
                spend_decision_id: decision_id.clone(),
                expires_at,
                used_at: None,
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

        Ok(SpendEvaluationResponse {
            decision_id,
            evaluation,
            auth_token,
        })
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
    use hubu_common::ids::AgentId;

    use super::*;
    use crate::policy::condition::{Condition, Field, PolicyValue};
    use hubu_common::money::Currency;

    use crate::policy::model::Rule;

    fn spend_request(amount_cents: i64) -> SpendRequest {
        SpendRequest {
            amount_cents,
            currency: Currency::Usd,
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
            rules,
            default_effect,
        }
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
            .evaluate_spend(request.clone(), &policy)
            .expect("spend evaluation should succeed");

        assert_eq!(response.evaluation.decision, Effect::Allow);
        assert_eq!(manager.decisions.len(), 1);
        assert_eq!(manager.tokens.len(), 1);

        let decision_record = manager
            .decisions
            .get(&response.decision_id)
            .expect("decision record should be stored");
        assert_eq!(decision_record.id, response.decision_id);
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
        assert_eq!(token_record.spend_decision_id, response.decision_id);
        assert_eq!(token_record.expires_at, token.expires_at);
        assert_eq!(token_record.used_at, None);
        assert_eq!(token_record.revoked_at, None);
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
            .evaluate_spend(request, &policy)
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
            rules: Vec::new(),
            default_effect: Effect::Allow,
        };

        let error = manager
            .evaluate_spend(request, &policy)
            .expect_err("invalid policy should fail spend evaluation");

        assert!(matches!(error, SpendError::PolicyValidation { .. }));
        assert_eq!(manager.decisions.len(), 0);
        assert_eq!(manager.tokens.len(), 0);
    }
}
