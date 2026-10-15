//! Source-owned automatic governance producers. Public HTTP inputs cannot
//! construct lifecycle facts; canonical transitions and their facts commit together.
use super::*;
use crate::policy::{Condition, Effect, Field, PolicyValue};
use crate::spend::lifecycle::*;

/// Immutable selected payload and public provenance travel together from the
/// assignment cache. Never recover provenance later from reusable authored labels.
#[derive(Debug, Clone)]
pub struct SelectedSpendPolicy {
    policy: Policy,
    policy_id: String,
    revision: u64,
    payload_hash: String,
}
impl From<PolicyAssignmentRecord> for SelectedSpendPolicy {
    fn from(value: PolicyAssignmentRecord) -> Self {
        Self {
            policy: value.policy,
            policy_id: value.policy_id,
            revision: value.policy_revision,
            payload_hash: value.policy_hash,
        }
    }
}
impl SelectedSpendPolicy {
    pub fn policy(&self) -> &Policy {
        &self.policy
    }
}

pub(super) fn unavailable<T>(reason: EvidenceUnavailableReason) -> LifecycleEvidence<T> {
    LifecycleEvidence::Unavailable { reason }
}
fn known<T>(value: T) -> LifecycleEvidence<T> {
    LifecycleEvidence::Known { value }
}
fn money(amount: Amount, currency: Currency) -> LifecycleAmount {
    LifecycleAmount { amount, currency }
}
fn invalid(message: &str) -> StorageError {
    StorageError::InvalidData(message.into())
}

fn policy_evidence(
    conn: &Connection,
    decision: &SpendDecisionRecord,
    selected: Option<&SelectedSpendPolicy>,
) -> Result<LifecycleEvidence<LifecyclePolicyEvidence>, StorageError> {
    let Some(selected) = selected else {
        return Ok(unavailable(EvidenceUnavailableReason::NotCaptured));
    };
    let stored: Option<String> = conn.query_row("SELECT r.policy_json FROM policy_revisions r JOIN policies p ON p.policy_id=r.policy_id WHERE p.owner_user_id=?1 AND r.policy_id=?2 AND r.revision=?3 AND r.payload_hash=?4", params![decision.owner_user_id.to_string(), selected.policy_id, selected.revision, selected.payload_hash], |row| row.get(0)).optional()?;
    let stored: Policy = serde_json::from_str(
        &stored.ok_or_else(|| invalid("selected policy revision is unavailable"))?,
    )?;
    // Whole payload comparison binds the actual evaluated snapshot, including
    // defaults/unmatched rules; authored id/version alone is not unique.
    if serde_json::to_value(&stored)? != serde_json::to_value(&selected.policy)?
        || serde_json::to_value(
            crate::policy::engine::evaluate_policy(&decision.request, &stored)
                .map_err(|_| invalid("selected policy cannot be evaluated"))?,
        )? != serde_json::to_value(&decision.evaluation)?
    {
        return Err(invalid("selected policy differs from evaluated policy"));
    }
    let mut evidence = LifecyclePolicyEvidence {
        policy_id: selected.policy_id.clone(),
        revision: selected.revision,
        payload_hash: selected.payload_hash.clone(),
        decisive_rule_ids: Vec::new(),
        amount_constraints: Vec::new(),
    };
    for rule in &selected.policy.rules {
        if rule.effect != decision.evaluation.decision
            || !rule.when.eval(&decision.request)
            || !super::lifecycle::stable_identifier(&rule.id)
            || selected
                .policy
                .rules
                .iter()
                .filter(|r| r.id == rule.id)
                .count()
                != 1
        {
            continue;
        }
        evidence.decisive_rule_ids.push(rule.id.clone());
        constraints(
            &rule.when,
            &rule.id,
            &decision.request,
            &mut evidence.amount_constraints,
        );
    }
    Ok(known(evidence))
}
fn constraints(
    condition: &Condition,
    rule_id: &str,
    request: &SpendRequest,
    out: &mut Vec<LifecycleAmountConstraint>,
) {
    let (comparison, field, value) = match condition {
        Condition::All { conditions } => {
            for c in conditions {
                constraints(c, rule_id, request, out);
            }
            return;
        }
        Condition::Eq { field, value } => (AmountComparison::Eq, field, value),
        Condition::Neq { field, value } => (AmountComparison::Neq, field, value),
        Condition::Gt { field, value } => (AmountComparison::Gt, field, value),
        Condition::Gte { field, value } => (AmountComparison::Gte, field, value),
        Condition::Lt { field, value } => (AmountComparison::Lt, field, value),
        Condition::Lte { field, value } => (AmountComparison::Lte, field, value),
        _ => return,
    };
    if *field != Field::Amount {
        return;
    }
    let amount = match value {
        PolicyValue::Money(v) => *v,
        PolicyValue::MoneyCents(v) => match Amount::from_legacy_cents(*v) {
            Ok(v) => v,
            Err(_) => return,
        },
        _ => return,
    };
    if amount.is_negative() {
        return;
    }
    out.push(LifecycleAmountConstraint {
        rule_id: rule_id.to_string(),
        comparison,
        configured: money(amount, request.currency),
        evaluated: known(money(request.amount, request.currency)),
    });
}
pub(super) fn budget_evidence(
    before: Option<&BudgetWithBalance>,
    request: &SpendRequest,
) -> LifecycleEvidence<LifecycleBudgetEvidence> {
    match before {
        None => unavailable(EvidenceUnavailableReason::NotCaptured),
        Some(b) => known(LifecycleBudgetEvidence {
            budget_id: format!("bgt_{}", b.budget.id.public_suffix()),
            budget_version_id: format!("bgv_{}", b.version.id.public_suffix()),
            requested: money(request.amount, request.currency),
            limit: known(money(b.version.amount_limit, b.budget.currency)),
            available: known(money(b.balance.remaining_amount, b.budget.currency)),
            consumed: known(money(b.balance.consumed_amount, b.budget.currency)),
            frozen: known(money(b.balance.frozen_amount, b.budget.currency)),
        }),
    }
}
pub(super) fn event(
    decision: &SpendDecisionRecord,
    fact: LifecycleFact,
    source: LifecycleSource,
    at: DateTime<Utc>,
    policy: LifecycleEvidence<LifecyclePolicyEvidence>,
    budget: LifecycleEvidence<LifecycleBudgetEvidence>,
) -> LifecycleEventInput {
    LifecycleEventInput {
        schema_version: LIFECYCLE_SCHEMA_VERSION,
        correlation: LifecycleCorrelation {
            owner_user_id: decision.owner_user_id.clone(),
            agent_id: decision.request.agent_id.clone(),
            agent_account_id: decision.request.agent_account_id.clone(),
            operation_key: decision.operation_key.clone(),
            attempt_revision: decision.revision,
            decision_id: Some(decision.id.clone()),
            correction_of_revision: decision.revision.checked_sub(1).filter(|v| *v > 0),
        },
        source,
        occurred_at: at,
        requested_amount: known(money(decision.request.amount, decision.request.currency)),
        execution_scope: match &decision.request.execution_scope {
            Some(s) => known(LifecycleScopeEvidence {
                provider: s.provider.id.clone(),
                executor: s.executor.id.clone(),
                capability: s.capability.id.clone(),
                billing_merchant: s.billing_merchant.id.clone(),
            }),
            None => unavailable(EvidenceUnavailableReason::NotCaptured),
        },
        fact,
        explanation: LifecycleExplanation { policy, budget },
    }
}
fn activate(conn: &Connection, owner: &UserId, at: DateTime<Utc>) -> Result<(), StorageError> {
    for source in [
        LifecycleSource::PolicyEngine,
        LifecycleSource::AutomatedAdmission,
    ] {
        super::lifecycle::ensure_lifecycle_capture_enabled_in_transaction(conn, owner, source, at)?;
    }
    Ok(())
}
fn insert_decision(conn: &Connection, record: &SpendDecisionRecord) -> Result<(), StorageError> {
    conn.execute("INSERT INTO spend_decisions(id,owner_user_id,agent_id,operation_key,revision,actor,request_json,evaluation_json,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![record.id.to_string(),record.owner_user_id.to_string(),record.request.agent_id.to_string(),record.operation_key,record.revision,record.actor,serde_json::to_string(&record.request)?,serde_json::to_string(&record.evaluation)?,record.created_at.to_rfc3339()])?;
    Ok(())
}
pub(super) fn save_policy(
    repo: &mut SqliteGovernanceRepository,
    decision: &SpendDecisionRecord,
    selected: Option<&SelectedSpendPolicy>,
) -> Result<(), StorageError> {
    let tx = repo
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    let policy = policy_evidence(&tx, decision, selected)?;
    insert_decision(&tx, decision)?;
    activate(&tx, &decision.owner_user_id, decision.created_at)?;
    let initial = match decision.evaluation.decision {
        Effect::Allow => InitialPolicyDecision::Allow,
        Effect::Deny => InitialPolicyDecision::Deny,
        Effect::NeedsApproval => InitialPolicyDecision::NeedsApproval,
    };
    append_lifecycle_event_in_transaction(
        &tx,
        &event(
            decision,
            LifecycleFact::InitialPolicy { decision: initial },
            LifecycleSource::PolicyEngine,
            decision.created_at,
            policy.clone(),
            unavailable(EvidenceUnavailableReason::NotApplicable),
        ),
    )?;
    if initial == InitialPolicyDecision::NeedsApproval {
        super::human_lifecycle::save_escalation(&tx, decision, policy)?;
    }
    tx.commit()?;
    Ok(())
}
pub(super) fn validate_before(
    conn: &Connection,
    before: &BudgetWithBalance,
    at: DateTime<Utc>,
) -> Result<(), StorageError> {
    let current = load_current_budget_snapshot(conn, &before.budget.id)?
        .ok_or_else(|| invalid("automatic admission budget snapshot is stale"))?;
    if current.version.id != before.version.id
        || current.version.amount_limit != before.version.amount_limit
        || current.balance.consumed_amount != before.balance.consumed_amount
        || current.balance.frozen_amount != before.balance.frozen_amount
        || current.balance.remaining_amount != before.balance.remaining_amount
        || current.budget.agent_id != before.budget.agent_id
        || current.budget.currency != before.budget.currency
        || current.budget.administrative_state != before.budget.administrative_state
        || current.budget.period.starting_at != before.budget.period.starting_at
        || current.budget.period.ending_before != before.budget.period.ending_before
        || current
            .availability_at(at)
            .map_err(|_| invalid("invalid admission budget eligibility"))?
            != before
                .availability_at(at)
                .map_err(|_| invalid("invalid admission budget eligibility"))?
    {
        return Err(invalid("automatic admission budget snapshot is stale"));
    }
    Ok(())
}
fn validate_no_available_budget(
    conn: &Connection,
    decision: &SpendDecisionRecord,
    at: DateTime<Utc>,
) -> Result<(), StorageError> {
    // Recheck the same own-currency selection under the SQLite write lock.
    // A budget in another currency cannot admit or explain this request.
    for budget in load_budgets_from(conn)? {
        if budget.agent_id != decision.request.agent_id
            || budget.currency != decision.request.currency
        {
            continue;
        }
        let current = load_current_budget_snapshot(conn, &budget.id)?
            .ok_or_else(|| invalid("missing admission budget snapshot"))?;
        if current
            .availability_at(at)
            .map_err(|_| invalid("invalid admission budget eligibility"))?
            .allows_reservation()
        {
            return Err(invalid("automatic admission budget selection is stale"));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn save_denied(
    repo: &mut SqliteGovernanceRepository,
    decision: &SpendDecisionRecord,
    selected: Option<&SelectedSpendPolicy>,
    before: Option<&BudgetWithBalance>,
    cause: AdmissionDenialReason,
    reasons: &[String],
    at: DateTime<Utc>,
    budget_checked_at: DateTime<Utc>,
) -> Result<(), StorageError> {
    // Only this source-owned typed branch can supply the terminal cause.
    match cause {
        AdmissionDenialReason::Policy if decision.evaluation.decision == Effect::Deny => {}
        AdmissionDenialReason::NoAvailableBudget
            if decision.evaluation.decision == Effect::Allow
                && before.is_none_or(|b| {
                    b.budget.currency == decision.request.currency
                        && b.availability_at(budget_checked_at)
                            .is_ok_and(|a| a == BudgetAvailability::Exhausted)
                }) => {}
        AdmissionDenialReason::InsufficientBudget
            if decision.evaluation.decision == Effect::Allow
                && before.is_some_and(|b| {
                    b.budget.currency == decision.request.currency
                        && b.balance.remaining_amount < decision.request.amount
                        && b.availability_at(budget_checked_at)
                            .is_ok_and(|a| a.allows_reservation())
                }) => {}
        _ => return Err(invalid("automatic denial does not match its transition")),
    }
    let tx = repo
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(before) = before {
        validate_before(&tx, before, budget_checked_at)?;
    }
    if cause == AdmissionDenialReason::NoAvailableBudget {
        validate_no_available_budget(&tx, decision, budget_checked_at)?;
    }
    let policy = policy_evidence(&tx, decision, selected)?;
    tx.execute("INSERT INTO spend_authorization_outcomes(agent_id,operation_key,revision,spend_decision_id,decision,reasons_json,created_at) VALUES(?1,?2,?3,?4,'denied',?5,?6)",params![decision.request.agent_id.to_string(),decision.operation_key,decision.revision,decision.id.to_string(),serde_json::to_string(reasons)?,at.to_rfc3339()])?;
    activate(&tx, &decision.owner_user_id, at)?;
    append_lifecycle_event_in_transaction(
        &tx,
        &event(
            decision,
            LifecycleFact::AutomatedAdmission {
                result: AutomatedAdmissionOutcome::Denied { reason: cause },
            },
            LifecycleSource::AutomatedAdmission,
            at,
            policy,
            if cause == AdmissionDenialReason::Policy {
                unavailable(EvidenceUnavailableReason::NotApplicable)
            } else {
                budget_evidence(before, &decision.request)
            },
        ),
    )?;
    tx.commit()?;
    Ok(())
}
#[allow(clippy::too_many_arguments)]
pub(super) fn save_approved(
    repo: &mut SqliteGovernanceRepository,
    decision: &SpendDecisionRecord,
    selected: Option<&SelectedSpendPolicy>,
    token: &SpendAuthTokenRecord,
    hold: &BudgetHold,
    balance: &BudgetBalance,
    before: &BudgetWithBalance,
    at: DateTime<Utc>,
    budget_checked_at: DateTime<Utc>,
) -> Result<(), StorageError> {
    if decision.evaluation.decision != Effect::Allow
        || hold.budget_version_id != before.version.id
        || !before
            .availability_at(budget_checked_at)
            .is_ok_and(|a| a.allows_reservation())
    {
        return Err(invalid(
            "automatic grant does not match its policy or budget",
        ));
    }
    let tx = repo
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    validate_before(&tx, before, budget_checked_at)?;
    let policy = policy_evidence(&tx, decision, selected)?;
    persist_approved_spend_transition(
        &tx,
        decision,
        token,
        hold,
        balance,
        &decision.evaluation.reasons,
        at,
    )?;
    activate(&tx, &decision.owner_user_id, at)?;
    let budget = budget_evidence(Some(before), &decision.request);
    for fact in [
        LifecycleFact::AutomatedAdmission {
            result: AutomatedAdmissionOutcome::Admitted,
        },
        LifecycleFact::AuthorizationGranted {
            authorization_id: decision.id.clone(),
        },
    ] {
        append_lifecycle_event_in_transaction(
            &tx,
            &event(
                decision,
                fact,
                LifecycleSource::AutomatedAdmission,
                at,
                policy.clone(),
                budget.clone(),
            ),
        )?;
    }
    tx.commit()?;
    Ok(())
}
pub(super) fn save_failure(
    repo: &mut SqliteGovernanceRepository,
    decision: &SpendDecisionRecord,
    observation: &LifecycleObservationId,
    at: DateTime<Utc>,
) -> Result<(), StorageError> {
    let tx = repo
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    activate(&tx, &decision.owner_user_id, at)?;
    append_lifecycle_event_in_transaction(
        &tx,
        &event(
            decision,
            LifecycleFact::AdmissionFailed {
                observation_id: observation.clone(),
            },
            LifecycleSource::AutomatedAdmission,
            at,
            unavailable(EvidenceUnavailableReason::NotCaptured),
            unavailable(EvidenceUnavailableReason::NotCaptured),
        ),
    )?;
    tx.commit()?;
    Ok(())
}

pub(super) fn save_evaluation_failure(
    repo: &mut SqliteGovernanceRepository,
    request: &SpendRequest,
    operation_key: &str,
    revision: u64,
    observation: &LifecycleObservationId,
    occurred_at: DateTime<Utc>,
) -> Result<(), StorageError> {
    let tx = repo
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    let input = LifecycleEventInput {
        schema_version: LIFECYCLE_SCHEMA_VERSION,
        correlation: LifecycleCorrelation {
            owner_user_id: request.owner_user_id.clone(),
            agent_id: request.agent_id.clone(),
            agent_account_id: request.agent_account_id.clone(),
            operation_key: operation_key.into(),
            attempt_revision: revision,
            decision_id: None,
            correction_of_revision: revision.checked_sub(1).filter(|v| *v > 0),
        },
        source: LifecycleSource::AutomatedAdmission,
        occurred_at,
        requested_amount: known(money(request.amount, request.currency)),
        execution_scope: match &request.execution_scope {
            Some(s) => known(LifecycleScopeEvidence {
                provider: s.provider.id.clone(),
                executor: s.executor.id.clone(),
                capability: s.capability.id.clone(),
                billing_merchant: s.billing_merchant.id.clone(),
            }),
            None => unavailable(EvidenceUnavailableReason::NotCaptured),
        },
        fact: LifecycleFact::AdmissionFailed {
            observation_id: observation.clone(),
        },
        explanation: LifecycleExplanation::default(),
    };
    activate(&tx, &request.owner_user_id, occurred_at)?;
    append_lifecycle_event_in_transaction(&tx, &input)?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
#[path = "automatic_lifecycle_tests.rs"]
mod tests;
