//! Versioned lifecycle evidence, separate from spend/approval/accounting state.
//!
//! Internal storage types, not public API DTOs: correlation contains a private
//! operation key. Exact replay adds no fact; corrections use a new attempt
//! revision. Period counts use occurred_at; backlog uses all visible open and
//! resolve facts through a recorded watermark, including pre-period opens.
use chrono::{DateTime, Utc};
use hubu_common::ids::{AgentAccountId, AgentId, SpendDecisionId, SpendExecutorClaimId, UserId};
use hubu_common::money::{Amount, Currency};
use serde::{Deserialize, Serialize};

pub const LIFECYCLE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleCorrelation {
    pub owner_user_id: UserId,
    pub agent_id: AgentId,
    pub agent_account_id: AgentAccountId,
    pub operation_key: String,
    pub attempt_revision: u64,
    pub decision_id: Option<SpendDecisionId>,
    /// Earlier revision of this operation, never an overwritten attempt.
    pub correction_of_revision: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleSource {
    PolicyEngine,
    AutomatedAdmission,
    HumanApproval,
    Executor,
    Reconciliation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InitialPolicyDecision {
    Allow,
    Deny,
    NeedsApproval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionDenialReason {
    Policy,
    NoAvailableBudget,
    InsufficientBudget,
    BudgetUnavailable,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum AutomatedAdmissionOutcome {
    Admitted,
    Denied { reason: AdmissionDenialReason },
}

/// Identity of one retryable technical observation, retained by the producer
/// on redelivery. Retrying execution may create a new observation, not a new
/// spend revision or a contradictory terminal admission result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleObservationId(SpendDecisionId);
impl LifecycleObservationId {
    pub fn new() -> Self {
        Self(SpendDecisionId::new())
    }
}
impl Default for LifecycleObservationId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanResolution {
    Approved,
    Declined,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionFinalization {
    Settled,
    Released,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationResolution {
    VendorBilled,
    VendorDidNotBill,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationReleaseReason {
    ExecutorRejectedBeforeClaim,
    Expired,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LifecycleFact {
    InitialPolicy {
        decision: InitialPolicyDecision,
    },
    AutomatedAdmission {
        result: AutomatedAdmissionOutcome,
    },
    AdmissionFailed {
        observation_id: LifecycleObservationId,
    },
    /// Only a real approval request, never a placeholder pending row or an
    /// inference from needs_approval alone.
    HumanEscalation {
        approval_request_id: SpendDecisionId,
    },
    /// Accepted durable closure of this immutable approval request. A human
    /// approval attempt that fails downstream and remains pending is not a
    /// confirmed resolution. This fact never synthesizes an authorization fact.
    HumanResolutionConfirmed {
        approval_request_id: SpendDecisionId,
        resolution: HumanResolution,
        confirmed_by_user_id: UserId,
    },
    AuthorizationGranted {
        authorization_id: SpendDecisionId,
    },
    /// Safe release before any executor claim; does not invent execution.
    AuthorizationReleased {
        authorization_id: SpendDecisionId,
        reason: AuthorizationReleaseReason,
    },
    ExecutionStarted {
        claim_id: SpendExecutorClaimId,
    },
    /// Execution failure can precede uncertain billing and later financial
    /// finalization. It never consumes the immutable finalization slot.
    ExecutionFailed {
        claim_id: SpendExecutorClaimId,
    },
    ExecutionFinalized {
        claim_id: SpendExecutorClaimId,
        outcome: ExecutionFinalization,
    },
    ReconciliationRequired {
        claim_id: SpendExecutorClaimId,
    },
    ReconciliationResolved {
        claim_id: SpendExecutorClaimId,
        outcome: ReconciliationResolution,
    },
}

/// Discrete occurred-time facts. Approval is not authorization; a policy allow
/// is not admission or spend. Distinct corrected revisions remain distinct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleCount {
    PolicyAllow,
    PolicyDeny,
    PolicyNeedsApproval,
    AdmissionAllowed,
    AdmissionDenied,
    AdmissionFailed,
    HumanEscalation,
    HumanApproved,
    HumanDeclined,
    AuthorizationGranted,
    AuthorizationReleased,
    ExecutionStarted,
    ExecutionSettled,
    ExecutionReleased,
    ExecutionFailed,
    ReconciliationRequired,
    ReconciliationResolved,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LifecycleBacklogSubject {
    HumanApproval(SpendDecisionId),
    Reconciliation(SpendExecutorClaimId),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleBacklogTransition {
    Open(LifecycleBacklogSubject),
    Resolve(LifecycleBacklogSubject),
}

impl LifecycleFact {
    pub fn count_category(&self) -> LifecycleCount {
        use LifecycleCount as C;
        match self {
            Self::InitialPolicy { decision } => match decision {
                InitialPolicyDecision::Allow => C::PolicyAllow,
                InitialPolicyDecision::Deny => C::PolicyDeny,
                InitialPolicyDecision::NeedsApproval => C::PolicyNeedsApproval,
            },
            Self::AutomatedAdmission { result } => match result {
                AutomatedAdmissionOutcome::Admitted => C::AdmissionAllowed,
                AutomatedAdmissionOutcome::Denied { .. } => C::AdmissionDenied,
            },
            Self::AdmissionFailed { .. } => C::AdmissionFailed,
            Self::HumanEscalation { .. } => C::HumanEscalation,
            Self::HumanResolutionConfirmed { resolution, .. } => match resolution {
                HumanResolution::Approved => C::HumanApproved,
                HumanResolution::Declined => C::HumanDeclined,
            },
            Self::AuthorizationGranted { .. } => C::AuthorizationGranted,
            Self::AuthorizationReleased { .. } => C::AuthorizationReleased,
            Self::ExecutionStarted { .. } => C::ExecutionStarted,
            Self::ExecutionFailed { .. } => C::ExecutionFailed,
            Self::ExecutionFinalized { outcome, .. } => match outcome {
                ExecutionFinalization::Settled => C::ExecutionSettled,
                ExecutionFinalization::Released => C::ExecutionReleased,
            },
            Self::ReconciliationRequired { .. } => C::ReconciliationRequired,
            Self::ReconciliationResolved { .. } => C::ReconciliationResolved,
        }
    }

    /// Resolve dominates open for an immutable subject even if an old open is
    /// recorded late. Missing predecessors mean partial coverage, not invented
    /// escalation/reconciliation facts. Recorded sequence only freezes visibility.
    pub fn backlog_transition(&self) -> Option<LifecycleBacklogTransition> {
        use LifecycleBacklogSubject as S;
        use LifecycleBacklogTransition as T;
        match self {
            Self::HumanEscalation {
                approval_request_id,
            } => Some(T::Open(S::HumanApproval(approval_request_id.clone()))),
            Self::HumanResolutionConfirmed {
                approval_request_id,
                ..
            } => Some(T::Resolve(S::HumanApproval(approval_request_id.clone()))),
            Self::ReconciliationRequired { claim_id } => {
                Some(T::Open(S::Reconciliation(claim_id.clone())))
            }
            Self::ReconciliationResolved { claim_id, .. } => {
                Some(T::Resolve(S::Reconciliation(claim_id.clone())))
            }
            _ => None,
        }
    }

    pub(crate) fn slot(&self) -> String {
        match self {
            Self::InitialPolicy { .. } => "initial_policy".into(),
            Self::AutomatedAdmission { .. } => "automated_admission".into(),
            Self::AdmissionFailed { observation_id } => {
                format!("admission_failed:{}", observation_id.0)
            }
            Self::HumanEscalation {
                approval_request_id,
            } => format!("human_escalation:{approval_request_id}"),
            Self::HumanResolutionConfirmed {
                approval_request_id,
                ..
            } => format!("human_resolution:{approval_request_id}"),
            Self::AuthorizationGranted { authorization_id } => {
                format!("authorization:{authorization_id}")
            }
            Self::AuthorizationReleased {
                authorization_id, ..
            } => format!("authorization_released:{authorization_id}"),
            Self::ExecutionStarted { claim_id } => format!("execution_started:{claim_id}"),
            Self::ExecutionFailed { claim_id } => format!("execution_failed:{claim_id}"),
            Self::ExecutionFinalized { claim_id, .. } => format!("execution_finalized:{claim_id}"),
            Self::ReconciliationRequired { claim_id } => {
                format!("reconciliation_required:{claim_id}")
            }
            Self::ReconciliationResolved { claim_id, .. } => {
                format!("reconciliation_resolved:{claim_id}")
            }
        }
    }
    pub(crate) fn accepts_source(&self, source: LifecycleSource) -> bool {
        match self {
            Self::InitialPolicy { .. } => source == LifecycleSource::PolicyEngine,
            Self::AutomatedAdmission { .. } | Self::AdmissionFailed { .. } => {
                source == LifecycleSource::AutomatedAdmission
            }
            Self::HumanEscalation { .. } | Self::HumanResolutionConfirmed { .. } => {
                source == LifecycleSource::HumanApproval
            }
            Self::AuthorizationGranted { .. } => matches!(
                source,
                LifecycleSource::AutomatedAdmission | LifecycleSource::HumanApproval
            ),
            Self::AuthorizationReleased { .. } => matches!(
                source,
                LifecycleSource::AutomatedAdmission | LifecycleSource::Executor
            ),
            Self::ExecutionStarted { .. } | Self::ExecutionFailed { .. } => {
                source == LifecycleSource::Executor
            }
            Self::ExecutionFinalized { .. } => matches!(
                source,
                LifecycleSource::Executor | LifecycleSource::Reconciliation
            ),
            Self::ReconciliationRequired { .. } => matches!(
                source,
                LifecycleSource::Executor | LifecycleSource::Reconciliation
            ),
            Self::ReconciliationResolved { .. } => source == LifecycleSource::Reconciliation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "coverage", rename_all = "snake_case", deny_unknown_fields)]
pub enum LifecycleEvidence<T> {
    Known { value: T },
    Unavailable { reason: EvidenceUnavailableReason },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceUnavailableReason {
    LegacyNotCaptured,
    NotCaptured,
    NotApplicable,
    Redacted,
    Unsupported,
}

/// Signed accounting-scale value; negative remaining after an overrun is valid.
/// The storage encoding is scaled integer text, never floats or rounded cents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleAmount {
    #[serde(with = "accounting_amount")]
    pub amount: Amount,
    pub currency: Currency,
}
mod accounting_amount {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        value: &Amount,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Scaled {
            units: String,
            scale: u32,
        }
        Scaled {
            units: value.units().to_string(),
            scale: hubu_common::money::ACCOUNTING_SCALE,
        }
        .serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Amount, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Scaled {
            units: String,
            scale: u32,
        }
        let scaled = Scaled::deserialize(deserializer)?;
        if scaled.scale != hubu_common::money::ACCOUNTING_SCALE {
            return Err(serde::de::Error::custom(
                "unsupported lifecycle amount scale",
            ));
        }
        let units: i64 = scaled.units.parse().map_err(serde::de::Error::custom)?;
        if units.to_string() != scaled.units {
            return Err(serde::de::Error::custom("noncanonical lifecycle amount"));
        }
        Ok(Amount::from_units(units))
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecyclePolicyEvidence {
    pub policy_id: String,
    pub revision: u64,
    pub payload_hash: String,
    /// Safe partial subset of stored matching rules with the winning initial
    /// policy effect. Omitted/redacted IDs do not assert that no other rules
    /// were decisive. Never free-form reasons/raw condition expressions.
    pub decisive_rule_ids: Vec<String>,
    pub amount_constraints: Vec<LifecycleAmountConstraint>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmountComparison {
    Eq,
    Neq,
    Gt,
    Gte,
    Lt,
    Lte,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleAmountConstraint {
    pub rule_id: String,
    pub comparison: AmountComparison,
    pub configured: LifecycleAmount,
    pub evaluated: LifecycleEvidence<LifecycleAmount>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleBudgetEvidence {
    pub budget_id: String,
    pub budget_version_id: String,
    pub requested: LifecycleAmount,
    pub limit: LifecycleEvidence<LifecycleAmount>,
    pub available: LifecycleEvidence<LifecycleAmount>,
    pub consumed: LifecycleEvidence<LifecycleAmount>,
    pub frozen: LifecycleEvidence<LifecycleAmount>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleScopeEvidence {
    pub provider: String,
    pub executor: String,
    pub capability: String,
    pub billing_merchant: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleExplanation {
    pub policy: LifecycleEvidence<LifecyclePolicyEvidence>,
    pub budget: LifecycleEvidence<LifecycleBudgetEvidence>,
}
impl Default for LifecycleExplanation {
    fn default() -> Self {
        Self {
            policy: LifecycleEvidence::Unavailable {
                reason: EvidenceUnavailableReason::NotCaptured,
            },
            budget: LifecycleEvidence::Unavailable {
                reason: EvidenceUnavailableReason::NotCaptured,
            },
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleEventInput {
    pub schema_version: u32,
    pub correlation: LifecycleCorrelation,
    pub source: LifecycleSource,
    pub occurred_at: DateTime<Utc>,
    /// Request evidence is independent of why a decision was made. It may be
    /// unavailable for an early admission failure, never inferred from cents.
    pub requested_amount: LifecycleEvidence<LifecycleAmount>,
    /// Canonical IDs resolved by the existing trusted request path, never raw
    /// selectors, merchant prose, prompts, or provider request URLs.
    pub execution_scope: LifecycleEvidence<LifecycleScopeEvidence>,
    pub fact: LifecycleFact,
    pub explanation: LifecycleExplanation,
}
/// Deliberately not Serialize: contains private correlation, not a public DTO.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleRecord {
    pub sequence: u64,
    pub event_id: String,
    pub agent_public_id: String,
    pub account_public_id: String,
    pub recorded_at: DateTime<Utc>,
    pub event: LifecycleEventInput,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleCaptureState {
    Enabled,
    Disabled,
}
/// Producer declaration, not an inference from the first event. Enabling capture
/// never claims complete history or explanation coverage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleCaptureMarker {
    pub sequence: u64,
    pub schema_version: u32,
    pub source: LifecycleSource,
    pub state: LifecycleCaptureState,
    pub effective_at: DateTime<Utc>,
    pub recorded_at: DateTime<Utc>,
}
#[derive(Debug, Clone)]
pub struct LifecycleSnapshot {
    pub schema_version: u32,
    pub watermark: u64,
    pub events: Vec<LifecycleRecord>,
    /// Absent means capture not declared: HUB-291 installs no active producers.
    /// Fact presence is independent of per-fact explanation coverage.
    pub capture_markers: Vec<LifecycleCaptureMarker>,
}
