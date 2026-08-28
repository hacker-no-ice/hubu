use std::collections::HashMap;

use chrono::{DateTime, Utc};
use hubu_common::execution_scope::ExecutionScope;
use hubu_common::ids::{
    AgentAccountId, AgentId, PaymentId, SpendAuthTokenId, SpendDecisionId, SpendExecutorClaimId,
    UserId,
};
use hubu_common::money::Currency;
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};
use serde_json::Value;
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
    #[serde(default = "default_lease_profile")]
    pub lease_profile: String,
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

pub fn default_lease_profile() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseProfile {
    pub claim_ttl_seconds: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseConfig {
    pub authorization_ttl_seconds: i64,
    pub default_lease_profile: String,
    pub lease_profiles: HashMap<String, LeaseProfile>,
}

impl Default for LeaseConfig {
    fn default() -> Self {
        Self {
            authorization_ttl_seconds: 5 * 60,
            default_lease_profile: default_lease_profile(),
            lease_profiles: HashMap::from([(
                default_lease_profile(),
                LeaseProfile {
                    claim_ttl_seconds: 15 * 60,
                },
            )]),
        }
    }
}

impl LeaseConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.authorization_ttl_seconds <= 0 {
            return Err("authorization_ttl_seconds must be positive".to_string());
        }
        if !self
            .lease_profiles
            .contains_key(&self.default_lease_profile)
        {
            return Err(format!(
                "default lease profile `{}` is not configured",
                self.default_lease_profile
            ));
        }
        for (name, profile) in &self.lease_profiles {
            if name.trim().is_empty() {
                return Err("lease profile names cannot be empty".to_string());
            }
            if profile.claim_ttl_seconds <= 0 {
                return Err(format!(
                    "lease profile `{name}` claim_ttl_seconds must be positive"
                ));
            }
        }
        Ok(())
    }

    pub fn lease_profile(&self, name: &str) -> Option<&LeaseProfile> {
        self.lease_profiles.get(name)
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
    pub lease_profile: String,
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
            lease_profile: "default".to_string(),
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

pub const MAX_VENDOR_COST_DECIMAL_SCALE: u32 = 18;

/// Exact provider cost expressed as `amount * 10^-scale` major currency units.
///
/// Budget accounting is in cents. Conversion therefore rounds toward positive
/// infinity so Hubu never understates externally billed consumption.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SpendExecutorVendorCost {
    pub amount: i64,
    pub scale: u32,
    #[serde(deserialize_with = "deserialize_executor_currency")]
    pub currency: Currency,
}

fn deserialize_executor_currency<'de, D>(deserializer: D) -> Result<Currency, D::Error>
where
    D: Deserializer<'de>,
{
    let currency = String::deserialize(deserializer)?;
    if currency.eq_ignore_ascii_case("usd") {
        Ok(Currency::Usd)
    } else {
        Err(D::Error::custom(format!(
            "unsupported actual vendor cost currency `{currency}`"
        )))
    }
}

impl SpendExecutorVendorCost {
    pub fn conservative_budget_charge_cents(&self) -> Result<i64, &'static str> {
        if self.amount < 0 {
            return Err("actual vendor cost cannot be negative");
        }
        if self.scale > MAX_VENDOR_COST_DECIMAL_SCALE {
            return Err("actual vendor cost scale cannot exceed 18");
        }

        if self.scale <= 2 {
            let multiplier = 10_i64
                .checked_pow(2 - self.scale)
                .ok_or("actual vendor cost cannot be represented in budget cents")?;
            return self
                .amount
                .checked_mul(multiplier)
                .ok_or("actual vendor cost cannot be represented in budget cents");
        }

        let divisor = 10_i64
            .checked_pow(self.scale - 2)
            .ok_or("actual vendor cost cannot be represented in budget cents")?;
        let quotient = self.amount / divisor;
        if self.amount % divisor == 0 {
            Ok(quotient)
        } else {
            quotient
                .checked_add(1)
                .ok_or("actual vendor cost cannot be represented in budget cents")
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SpendExecutorSettlementReceipt {
    pub actual_vendor_cost: SpendExecutorVendorCost,
    pub provider_request_id: String,
    pub price_model_snapshot: Value,
    pub artifact_reference: String,
}

impl<'de> Deserialize<'de> for SpendExecutorSettlementReceipt {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct PreciseReceipt {
            actual_vendor_cost: SpendExecutorVendorCost,
            provider_request_id: String,
            price_model_snapshot: Value,
            artifact_reference: String,
        }

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct LegacyV43Receipt {
            actual_vendor_cost_cents: i64,
            provider_request_id: String,
            price_model_snapshot: Value,
            artifact_reference: String,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum CompatibleReceipt {
            Precise(PreciseReceipt),
            LegacyV43(LegacyV43Receipt),
        }

        Ok(match CompatibleReceipt::deserialize(deserializer)? {
            CompatibleReceipt::Precise(receipt) => Self {
                actual_vendor_cost: receipt.actual_vendor_cost,
                provider_request_id: receipt.provider_request_id,
                price_model_snapshot: receipt.price_model_snapshot,
                artifact_reference: receipt.artifact_reference,
            },
            CompatibleReceipt::LegacyV43(receipt) => Self {
                actual_vendor_cost: SpendExecutorVendorCost {
                    amount: receipt.actual_vendor_cost_cents,
                    scale: 2,
                    currency: Currency::Usd,
                },
                provider_request_id: receipt.provider_request_id,
                price_model_snapshot: receipt.price_model_snapshot,
                artifact_reference: receipt.artifact_reference,
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedSpendExecutorSettlementReceipt {
    pub claim_id: SpendExecutorClaimId,
    pub settlement_id: PaymentId,
    pub authorized_max_cents: i64,
    pub budget_charge_cents: i64,
    pub released_amount_cents: i64,
    pub overrun_amount_cents: i64,
    pub currency: Currency,
    pub receipt: SpendExecutorSettlementReceipt,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod executor_receipt_precision_tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn conservative_budget_conversion_covers_fractional_cent_boundaries() {
        let cost = |amount, scale| SpendExecutorVendorCost {
            amount,
            scale,
            currency: Currency::Usd,
        };

        assert_eq!(cost(1, 3).conservative_budget_charge_cents(), Ok(1));
        assert_eq!(cost(10, 3).conservative_budget_charge_cents(), Ok(1));
        assert_eq!(cost(11, 3).conservative_budget_charge_cents(), Ok(2));
        assert_eq!(
            cost(25_000, 3).conservative_budget_charge_cents(),
            Ok(2_500)
        );
        assert_eq!(
            cost(25_001, 3).conservative_budget_charge_cents(),
            Ok(2_501)
        );
        assert!(cost(1, 19).conservative_budget_charge_cents().is_err());
        assert!(cost(i64::MAX, 0)
            .conservative_budget_charge_cents()
            .is_err());
    }

    #[test]
    fn legacy_v43_receipt_maps_cents_to_exact_scale_two() {
        let legacy = json!({
            "actual_vendor_cost_cents": 17,
            "provider_request_id": "provider-request-17",
            "price_model_snapshot": {
                "provider": "provider",
                "model": "model",
                "currency": "usd",
                "nested": { "frozen": true }
            },
            "artifact_reference": "artifact://17"
        });
        let receipt: SpendExecutorSettlementReceipt = serde_json::from_value(legacy).unwrap();

        assert_eq!(
            receipt.actual_vendor_cost,
            SpendExecutorVendorCost {
                amount: 17,
                scale: 2,
                currency: Currency::Usd,
            }
        );
        assert_eq!(receipt.price_model_snapshot["nested"]["frozen"], true);
        let serialized = serde_json::to_value(receipt).unwrap();
        assert_eq!(serialized["actual_vendor_cost"]["amount"], 17);
        assert!(serialized.get("actual_vendor_cost_cents").is_none());
    }

    #[test]
    fn precise_receipt_rejects_mixed_legacy_and_exact_cost_fields() {
        let mixed = json!({
            "actual_vendor_cost": { "amount": 17, "scale": 3, "currency": "usd" },
            "actual_vendor_cost_cents": 2,
            "provider_request_id": "provider-request-17",
            "price_model_snapshot": {
                "provider": "provider",
                "model": "model",
                "currency": "usd"
            },
            "artifact_reference": "artifact://17"
        });
        assert!(serde_json::from_value::<SpendExecutorSettlementReceipt>(mixed).is_err());
    }

    #[test]
    fn precise_receipt_accepts_uppercase_iso_currency_and_serializes_canonically() {
        let exact = json!({
            "actual_vendor_cost": { "amount": 17, "scale": 3, "currency": "USD" },
            "provider_request_id": "provider-request-17",
            "price_model_snapshot": {
                "provider": "provider",
                "model": "model",
                "currency": "USD"
            },
            "artifact_reference": "artifact://17"
        });
        let receipt: SpendExecutorSettlementReceipt = serde_json::from_value(exact).unwrap();
        assert_eq!(receipt.actual_vendor_cost.currency, Currency::Usd);
        assert_eq!(
            serde_json::to_value(receipt).unwrap()["actual_vendor_cost"]["currency"],
            "usd"
        );
    }
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
