use std::collections::HashMap;

use chrono::{DateTime, Utc};
use hubu_common::execution_scope::ExecutionScope;
use hubu_common::ids::{
    AgentAccountId, AgentId, PaymentId, SpendAuthTokenId, SpendDecisionId, SpendExecutorClaimId,
    UserId,
};
use hubu_common::money::Currency;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::policy::model::Evaluation;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SpendRequest {
    pub amount_cents: i64, // in minor unit
    pub currency: Currency,
    pub owner_user_id: UserId,
    pub agent_id: AgentId,
    pub agent_account_id: AgentAccountId,
    pub merchant: Option<String>,
    #[serde(default)]
    pub execution_scope: Option<ExecutionScope>,
    pub category: Option<String>,
    pub task_id: Option<String>,
    #[serde(default)]
    pub reason: String,
    #[serde(default = "default_workload_profile")]
    pub workload_profile: String,
}

impl SpendRequest {
    pub(crate) fn replay_equivalent(&self, other: &Self) -> bool {
        let self_normalized = self.with_legacy_reason();
        let other_normalized = other.with_legacy_reason();
        if self_normalized == other_normalized {
            return true;
        }
        let (without_scope, with_scope) = match (
            &self_normalized.execution_scope,
            &other_normalized.execution_scope,
        ) {
            (None, Some(_)) => (&self_normalized, &other_normalized),
            (Some(_), None) => (&other_normalized, &self_normalized),
            _ => return false,
        };
        let Some(merchant) = with_scope.merchant.as_deref() else {
            return false;
        };
        let Some(scope) = with_scope.execution_scope.as_ref() else {
            return false;
        };
        if !is_legacy_scope_for_merchant(scope, merchant) {
            return false;
        }
        let mut normalized = with_scope.clone();
        normalized.execution_scope = None;
        without_scope == &normalized
    }

    fn with_legacy_reason(&self) -> Self {
        let mut normalized = self.clone();
        normalized.normalize_legacy_reason();
        normalized
    }

    pub(crate) fn normalize_legacy_reason(&mut self) {
        if self.reason.is_empty() {
            self.reason = self.task_id.clone().unwrap_or_default();
        }
    }
}

fn is_legacy_scope_for_merchant(scope: &ExecutionScope, merchant: &str) -> bool {
    let merchant = merchant.trim();
    let digest = format!("{:x}", Sha256::digest(merchant.as_bytes()));
    scope.schema_version == hubu_common::execution_scope::EXECUTION_SCOPE_SCHEMA_VERSION
        && scope.provider.id == "provider:legacy:unresolved"
        && scope.provider.display_name == "Legacy unresolved provider"
        && scope.executor.id == "executor:legacy:unresolved"
        && scope.executor.display_name == "Legacy unresolved executor"
        && scope.capability.id == "capability:legacy:unresolved"
        && scope.capability.display_name == "Legacy unresolved capability"
        && scope.billing_merchant.id == format!("merchant:legacy:{}", &digest[..16])
        && scope.billing_merchant.display_name == merchant
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
    pub revision: u64,
    pub actor: String,
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
    pub revision: u64,
    pub retry_guidance: SpendRetryGuidance,
    pub attempt_history: Vec<SpendAttemptAuditRecord>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SpendRetryAction {
    ReuseOperationKey,
    ReplayExactly,
    CreateNewOperation,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SpendRetryGuidance {
    pub action: SpendRetryAction,
    pub operation_key: String,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SpendAuthorizationDecision {
    PendingApproval,
    Denied,
    Allowed,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpendAttemptAuditRecord {
    pub revision: u64,
    pub request: SpendRequest,
    pub actor: String,
    pub submitted_at: DateTime<Utc>,
    pub decision_id: Option<SpendDecisionId>,
    pub final_decision: SpendAuthorizationDecision,
    pub decided_at: DateTime<Utc>,
    pub reasons: Vec<String>,
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
    pub execution_scope: Option<ExecutionScope>,
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
    pub provider_reference: Option<String>,
    pub reconciliation_evidence: Option<String>,
    pub reconciled_at: Option<DateTime<Utc>>,
    pub reconciled_by_user_id: Option<UserId>,
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    fn request(task_id: Option<&str>, reason: &str) -> SpendRequest {
        SpendRequest {
            amount_cents: 500,
            currency: Currency::Usd,
            owner_user_id: "00000000-0000-4000-8000-000000000123".parse().unwrap(),
            agent_id: AgentId::new(),
            agent_account_id: AgentAccountId::new(),
            merchant: Some("vendor.example".to_string()),
            execution_scope: None,
            category: None,
            task_id: task_id.map(str::to_string),
            reason: reason.to_string(),
            workload_profile: "default".to_string(),
        }
    }

    #[test]
    fn legacy_persisted_reason_maps_from_task_id_for_replay() {
        let legacy = request(Some("Generate logo"), "");
        let mut replay = legacy.clone();
        replay.reason = "Generate logo".to_string();
        assert!(legacy.replay_equivalent(&replay));
    }

    #[test]
    fn independent_task_and_reason_are_both_replay_bound() {
        let request = request(Some("linear:HUB-73"), "Generate logo");
        let mut changed_task = request.clone();
        changed_task.task_id = Some("linear:HUB-74".to_string());
        let mut changed_reason = request.clone();
        changed_reason.reason = "Generate a different logo".to_string();
        assert!(!request.replay_equivalent(&changed_task));
        assert!(!request.replay_equivalent(&changed_reason));
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SpendExecutorPriceModelSnapshot {
    pub provider: String,
    pub model: String,
    pub unit_price_cents: i64,
    pub pricing_unit: String,
    pub currency: Currency,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SpendExecutorSettlementReceipt {
    pub actual_vendor_cost_cents: i64,
    pub provider_request_id: String,
    pub price_model_snapshot: SpendExecutorPriceModelSnapshot,
    pub artifact_reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedSpendExecutorSettlementReceipt {
    pub claim_id: SpendExecutorClaimId,
    pub settlement_id: PaymentId,
    pub authorized_max_cents: i64,
    pub released_amount_cents: i64,
    pub currency: Currency,
    pub receipt: SpendExecutorSettlementReceipt,
    pub created_at: DateTime<Utc>,
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
