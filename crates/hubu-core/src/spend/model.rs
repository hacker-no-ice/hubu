use std::collections::HashMap;

use chrono::{DateTime, Utc};
use hubu_common::ids::{
    AgentAccountId, AgentId, PaymentId, SpendAuthTokenId, SpendDecisionId, SpendExecutorClaimId,
    UserId,
};
use hubu_common::money::Currency;
use serde::{Deserialize, Serialize};

use crate::policy::model::Evaluation;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SpendRequest {
    pub amount_cents: i64, // in minor unit
    pub currency: Currency,
    pub owner_user_id: UserId,
    pub agent_id: AgentId,
    pub agent_account_id: AgentAccountId,
    pub merchant: Option<String>,
    pub category: Option<String>,
    pub task_id: Option<String>,
    #[serde(default = "default_workload_profile")]
    pub workload_profile: String,
}

pub fn default_workload_profile() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SpendTimingProfile {
    pub authorization_ttl_seconds: i64,
    pub claim_ttl_seconds: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SpendTimingConfig {
    pub default_profile: String,
    pub profiles: HashMap<String, SpendTimingProfile>,
}

impl Default for SpendTimingConfig {
    fn default() -> Self {
        Self {
            default_profile: default_workload_profile(),
            profiles: HashMap::from([(
                default_workload_profile(),
                SpendTimingProfile {
                    authorization_ttl_seconds: 5 * 60,
                    claim_ttl_seconds: 15 * 60,
                },
            )]),
        }
    }
}

impl SpendTimingConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !self.profiles.contains_key(&self.default_profile) {
            return Err(format!(
                "default workload profile `{}` is not configured",
                self.default_profile
            ));
        }
        for (name, profile) in &self.profiles {
            if name.trim().is_empty() {
                return Err("workload profile names cannot be empty".to_string());
            }
            if profile.authorization_ttl_seconds <= 0 {
                return Err(format!(
                    "workload profile `{name}` authorization_ttl_seconds must be positive"
                ));
            }
            if profile.claim_ttl_seconds <= 0 {
                return Err(format!(
                    "workload profile `{name}` claim_ttl_seconds must be positive"
                ));
            }
        }
        Ok(())
    }

    pub fn profile(&self, name: &str) -> Option<&SpendTimingProfile> {
        self.profiles.get(name)
    }
}

/// immutable log for tracking a spend decision
#[derive(Debug, Clone)]
pub struct SpendDecisionRecord {
    pub id: SpendDecisionId,
    pub owner_user_id: UserId,
    pub operation_key: String,
    pub request: SpendRequest,
    pub evaluation: Evaluation,
    pub created_at: DateTime<Utc>,
}

/// record for tracking auth token issurance for allowed spend
#[derive(Debug, Clone)]
pub struct SpendAuthTokenRecord {
    pub id: SpendAuthTokenId,
    pub owner_user_id: UserId,
    pub spend_decision_id: SpendDecisionId,
    pub expires_at: DateTime<Utc>,
    pub claim_ttl_seconds: i64,
    pub used_at: Option<DateTime<Utc>>,
    pub used_by_payment_id: Option<PaymentId>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct SpendEvaluationResponse {
    pub operation_key: String,
    pub decision_id: SpendDecisionId,
    pub evaluation: Evaluation,
    pub auth_token: Option<IssuedSpendAuthToken>,
    pub idempotent_replay: bool,
}

#[derive(Debug, Clone)]
pub struct IssuedSpendAuthToken {
    pub id: SpendAuthTokenId,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct SpendPaymentValidationRequest {
    pub spend_auth_token_id: SpendAuthTokenId,
    pub owner_user_id: UserId,
    pub agent_id: AgentId,
    pub agent_account_id: AgentAccountId,
    pub amount_cents: i64,
    pub currency: Currency,
    pub merchant: Option<String>,
    pub task_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ValidatedSpendAuthorization {
    pub spend_auth_token_id: SpendAuthTokenId,
    pub owner_user_id: UserId,
    pub spend_decision_id: SpendDecisionId,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpendExecutorClaimStatus {
    Claimed,
    Settled,
    Released,
}

#[derive(Debug, Clone)]
pub struct SpendExecutorClaimRecord {
    pub id: SpendExecutorClaimId,
    pub spend_auth_token_id: SpendAuthTokenId,
    pub owner_user_id: UserId,
    pub agent_id: AgentId,
    pub operation_key: String,
    pub workload_profile: String,
    pub status: SpendExecutorClaimStatus,
    pub claimed_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub finalized_at: Option<DateTime<Utc>>,
    pub settlement_id: Option<PaymentId>,
}

#[derive(Debug, Clone)]
pub struct SpendExecutorClaimRequest {
    pub authorization: SpendPaymentValidationRequest,
    pub operation_key: String,
}

#[derive(Debug, Clone)]
pub struct SpendExecutorClaimValidationRequest {
    pub owner_user_id: UserId,
    pub agent_id: AgentId,
    pub operation_key: String,
}
