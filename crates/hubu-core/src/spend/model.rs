use chrono::{DateTime, Utc};
use hubu_common::ids::{AgentId, PaymentId, SpendAuthTokenId, SpendDecisionId};
use hubu_common::money::Currency;

use crate::policy::model::Evaluation;

#[derive(Debug, Clone)]
pub struct SpendRequest {
    pub amount_cents: i64, // in minor unit
    pub currency: Currency,
    pub agent_id: AgentId,
    pub merchant: Option<String>,
    pub category: Option<String>,
    pub task_id: Option<String>,
}

/// immutable log for tracking a spend decision
#[derive(Debug, Clone)]
pub struct SpendDecisionRecord {
    pub id: SpendDecisionId,
    pub request: SpendRequest,
    pub evaluation: Evaluation,
    pub created_at: DateTime<Utc>,
}

/// record for tracking auth token issurance for allowed spend
#[derive(Debug, Clone)]
pub struct SpendAuthTokenRecord {
    pub id: SpendAuthTokenId,
    pub spend_decision_id: SpendDecisionId,
    pub expires_at: DateTime<Utc>,
    pub used_at: Option<DateTime<Utc>>,
    pub used_by_payment_id: Option<PaymentId>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct SpendEvaluationResponse {
    pub decision_id: SpendDecisionId,
    pub evaluation: Evaluation,
    pub auth_token: Option<IssuedSpendAuthToken>,
}

#[derive(Debug, Clone)]
pub struct IssuedSpendAuthToken {
    pub id: SpendAuthTokenId,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct SpendPaymentValidationRequest {
    pub spend_auth_token_id: SpendAuthTokenId,
    pub agent_id: AgentId,
    pub amount_cents: i64,
    pub currency: Currency,
    pub merchant: Option<String>,
    pub task_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ValidatedSpendAuthorization {
    pub spend_auth_token_id: SpendAuthTokenId,
    pub spend_decision_id: SpendDecisionId,
    pub expires_at: DateTime<Utc>,
}
