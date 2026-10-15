//! Safe read models for captured governance evidence. Money stays typed until
//! an HTTP/MCP adapter maps it through the shared exact-money response helper.
//! A stage count is not a distinct attempt count: stages intentionally overlap.
use super::lifecycle::*;
use chrono::{DateTime, Utc};
use hubu_common::money::Currency;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GovernanceCategory {
    InitialPolicy,
    AutomatedAdmission,
    AdmissionFailure,
    HumanEscalation,
    HumanResolution,
    Authorization,
    AuthorizationRelease,
    ExecutionStart,
    ExecutionFailure,
    ExecutionFinalization,
    ReconciliationEntry,
    ReconciliationResolution,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GovernanceOutcome {
    PolicyAllow,
    PolicyDeny,
    PolicyNeedsApproval,
    AutomaticAllow,
    AutomaticDeny,
    AdmissionFailed,
    Escalated,
    HumanApproved,
    HumanDeclined,
    Authorized,
    AuthorizationReleased,
    ExecutionStarted,
    ExecutionFailed,
    ExecutionSettled,
    ExecutionReleased,
    ReconciliationRequired,
    ReconciliationResolved,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GovernanceDenialReason {
    Policy,
    NoAvailableBudget,
    InsufficientBudget,
    BudgetUnavailable,
    Unknown,
}

/// No implicit rolling time window. None means all captured history.
#[derive(Debug, Clone, Default)]
pub struct GovernanceQuery {
    pub agent_public_id: Option<String>,
    pub account_public_id: Option<String>,
    pub categories: Vec<GovernanceCategory>,
    pub outcomes: Vec<GovernanceOutcome>,
    pub occurred_from: Option<DateTime<Utc>>,
    pub occurred_before: Option<DateTime<Utc>>,
    pub snapshot: Option<String>,
    pub cursor: Option<String>,
    /// List page size, 1..=200; None uses 50. Summary/detail reject a cursor.
    pub limit: Option<usize>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceScope {
    pub agent_public_id: Option<String>,
    pub account_public_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceSnapshot {
    pub token: String,
    pub watermark: u64,
    /// The durable boundary's DB recorded_at, never a caller supplied clock.
    pub captured_evidence_as_of: Option<DateTime<Utc>>,
}
/// Explicit allowlist. This type cannot serialize private lifecycle correlation,
/// actor IDs, operation keys, provider payloads, tokens, URLs or free prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceFact {
    pub event_public_id: String,
    pub agent_public_id: String,
    pub account_public_id: String,
    pub occurred_at: DateTime<Utc>,
    pub recorded_at: DateTime<Utc>,
    pub attempt_revision: u64,
    pub category: GovernanceCategory,
    pub outcome: GovernanceOutcome,
    pub source: LifecycleSource,
    pub authorization_release_reason: Option<AuthorizationReleaseReason>,
    pub reconciliation_resolution: Option<ReconciliationResolution>,
    pub automatic_denial_reason: Option<GovernanceDenialReason>,
    pub requested_amount: LifecycleEvidence<LifecycleAmount>,
    pub execution_scope: LifecycleEvidence<LifecycleScopeEvidence>,
    pub policy: LifecycleEvidence<LifecyclePolicyEvidence>,
    pub budget: LifecycleEvidence<LifecycleBudgetEvidence>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceOutcomeCount {
    pub outcome: GovernanceOutcome,
    pub fact_count: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceDenialCount {
    pub reason: GovernanceDenialReason,
    pub distinct_attempts: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceCurrencyTotal {
    pub currency: Currency,
    /// Accounting-scale units, checked i128 accumulation from Amount::units.
    pub units: i128,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GovernanceRequestedTotal {
    pub known_attempts: u64,
    pub unknown_attempts: u64,
    pub per_currency: Vec<GovernanceCurrencyTotal>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceExplanationCoverage {
    pub category: GovernanceCategory,
    pub facts: u64,
    pub known_requested_amount: u64,
    pub known_policy: u64,
    pub known_budget: u64,
    pub known_execution_scope: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceCaptureDeclaration {
    pub source: LifecycleSource,
    pub state: LifecycleCaptureState,
    pub effective_at: DateTime<Utc>,
    pub recorded_at: DateTime<Utc>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceCategoryCoverage {
    pub category: GovernanceCategory,
    /// Support is per fact, not inherited from an enabled source. In particular
    /// provider execution failures remain unsupported.
    pub supported: bool,
    pub capture_declarations: Vec<GovernanceCaptureDeclaration>,
    /// Captured evidence is partial history, including when observed_count=0.
    pub complete_history: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GovernanceUnsupportedSignal {
    ProviderExecutionFailure,
    PreExpiryBillingUncertainty,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceBacklog {
    /// Identity scope only. Independent of occurred-time/category/outcome filters.
    pub scope: GovernanceScope,
    pub captured_evidence_as_of: Option<DateTime<Utc>>,
    pub pending_human_approvals: u64,
    pub unresolved_reconciliations: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceSummary {
    pub scope: GovernanceScope,
    pub snapshot: GovernanceSnapshot,
    pub fact_count: u64,
    pub distinct_attempts: u64,
    pub distinct_operations: u64,
    /// Counts overlap across stages; they must not be added to count attempts.
    pub stage_counts_overlap: bool,
    pub stages: Vec<GovernanceOutcomeCount>,
    /// One primary typed admission denial per denied attempt, never one per rule.
    pub automatic_denials: Vec<GovernanceDenialCount>,
    pub requested: GovernanceRequestedTotal,
    /// Blocked request evidence, not cost, savings, consumption or settlement.
    pub automatic_denied_requested: GovernanceRequestedTotal,
    pub backlog: GovernanceBacklog,
    pub record_coverage: Vec<GovernanceCategoryCoverage>,
    /// Reconciliation capture covers Hubu financial lease expiry and confirmed
    /// closure; it does not claim provider-side or pre-expiry observations.
    pub unsupported_signals: Vec<GovernanceUnsupportedSignal>,
    pub explanation_coverage: Vec<GovernanceExplanationCoverage>,
    pub legacy_coverage: GovernanceLegacyCoverage,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceLegacyCoverage {
    pub observed_at: Option<DateTime<Utc>>,
    /// False when there is no inventory or the selected identity was not assessed.
    pub assessed: bool,
    pub unlinked_metrics: Vec<LifecycleCount>,
    pub unlinked_human_approval_backlog: bool,
    pub unlinked_reconciliation_backlog: bool,
    /// Broader legacy evidence cannot be attributed to this selected identity.
    pub broader_attribution_unknown: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernancePage {
    pub facts: Vec<GovernanceFact>,
    /// Full-filter totals, independent of the current page and cursor.
    pub summary: GovernanceSummary,
    pub next_cursor: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceDetail {
    pub fact: GovernanceFact,
    pub summary: GovernanceSummary,
}

pub(crate) fn classify(fact: &LifecycleFact) -> (GovernanceCategory, GovernanceOutcome) {
    use GovernanceCategory as C;
    use GovernanceOutcome as O;
    match fact {
        LifecycleFact::InitialPolicy { decision } => (
            C::InitialPolicy,
            match decision {
                InitialPolicyDecision::Allow => O::PolicyAllow,
                InitialPolicyDecision::Deny => O::PolicyDeny,
                InitialPolicyDecision::NeedsApproval => O::PolicyNeedsApproval,
            },
        ),
        LifecycleFact::AutomatedAdmission { result } => (
            C::AutomatedAdmission,
            match result {
                AutomatedAdmissionOutcome::Admitted => O::AutomaticAllow,
                AutomatedAdmissionOutcome::Denied { .. } => O::AutomaticDeny,
            },
        ),
        LifecycleFact::AdmissionFailed { .. } => (C::AdmissionFailure, O::AdmissionFailed),
        LifecycleFact::HumanEscalation { .. } => (C::HumanEscalation, O::Escalated),
        LifecycleFact::HumanResolutionConfirmed { resolution, .. } => (
            C::HumanResolution,
            match resolution {
                HumanResolution::Approved => O::HumanApproved,
                HumanResolution::Declined => O::HumanDeclined,
            },
        ),
        LifecycleFact::AuthorizationGranted { .. } => (C::Authorization, O::Authorized),
        LifecycleFact::AuthorizationReleased { .. } => {
            (C::AuthorizationRelease, O::AuthorizationReleased)
        }
        LifecycleFact::ExecutionStarted { .. } => (C::ExecutionStart, O::ExecutionStarted),
        LifecycleFact::ExecutionFailed { .. } => (C::ExecutionFailure, O::ExecutionFailed),
        LifecycleFact::ExecutionFinalized { outcome, .. } => (
            C::ExecutionFinalization,
            match outcome {
                ExecutionFinalization::Settled => O::ExecutionSettled,
                ExecutionFinalization::Released => O::ExecutionReleased,
            },
        ),
        LifecycleFact::ReconciliationRequired { .. } => {
            (C::ReconciliationEntry, O::ReconciliationRequired)
        }
        LifecycleFact::ReconciliationResolved { .. } => {
            (C::ReconciliationResolution, O::ReconciliationResolved)
        }
    }
}
pub(crate) fn denial_reason(fact: &LifecycleFact) -> Option<GovernanceDenialReason> {
    let LifecycleFact::AutomatedAdmission {
        result: AutomatedAdmissionOutcome::Denied { reason },
    } = fact
    else {
        return None;
    };
    Some(match reason {
        AdmissionDenialReason::Policy => GovernanceDenialReason::Policy,
        AdmissionDenialReason::NoAvailableBudget => GovernanceDenialReason::NoAvailableBudget,
        AdmissionDenialReason::InsufficientBudget => GovernanceDenialReason::InsufficientBudget,
        AdmissionDenialReason::BudgetUnavailable => GovernanceDenialReason::BudgetUnavailable,
        AdmissionDenialReason::Unknown => GovernanceDenialReason::Unknown,
    })
}
