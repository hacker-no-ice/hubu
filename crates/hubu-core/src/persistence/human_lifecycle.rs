//! Source-owned evidence for genuine approval requests and their accepted closure.
//! A failed human action leaves the request pending and publishes no resolution.
use super::*;
use crate::policy::Effect;
use crate::spend::lifecycle::*;
use automatic_lifecycle::{budget_evidence, event, unavailable, validate_before};

fn invalid(message: &str) -> StorageError {
    StorageError::InvalidData(message.into())
}
fn activate(
    conn: &Connection,
    decision: &SpendDecisionRecord,
    at: DateTime<Utc>,
) -> Result<(), StorageError> {
    lifecycle::ensure_lifecycle_capture_enabled_in_transaction(
        conn,
        &decision.owner_user_id,
        LifecycleSource::HumanApproval,
        at,
    )?;
    Ok(())
}

/// Called only while saving the real immutable policy decision, never while
/// admitting the temporary evaluation-in-progress outcome.
pub(super) fn save_escalation(
    conn: &Connection,
    decision: &SpendDecisionRecord,
    policy: LifecycleEvidence<LifecyclePolicyEvidence>,
) -> Result<(), StorageError> {
    if decision.evaluation.decision != Effect::NeedsApproval {
        return Err(invalid(
            "human escalation requires an immutable approval request",
        ));
    }
    activate(conn, decision, decision.created_at)?;
    append_lifecycle_event_in_transaction(
        conn,
        &event(
            decision,
            LifecycleFact::HumanEscalation {
                approval_request_id: decision.id.clone(),
            },
            LifecycleSource::HumanApproval,
            decision.created_at,
            policy,
            unavailable(EvidenceUnavailableReason::NotApplicable),
        ),
    )?;
    Ok(())
}

/// Bind human closure to the actual pending immutable request under the same
/// write lock as its outcome. A stale second writer cannot overwrite closure.
fn require_pending(
    conn: &Connection,
    decision: &SpendDecisionRecord,
    confirmed_by: &UserId,
    at: DateTime<Utc>,
) -> Result<(), StorageError> {
    if decision.evaluation.decision != Effect::NeedsApproval
        || confirmed_by != &decision.owner_user_id
        || at < decision.created_at
    {
        return Err(invalid("invalid human approval confirmation"));
    }
    let stored: Option<(String, String, String, u64, String, String)> = conn.query_row(
        "SELECT owner_user_id,agent_id,operation_key,revision,request_json,evaluation_json FROM spend_decisions WHERE id=?1",
        [decision.id.to_string()],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)),
    ).optional()?;
    let Some((owner, agent, key, revision, request, evaluation)) = stored else {
        return Err(invalid("unknown human approval request"));
    };
    if owner != decision.owner_user_id.to_string()
        || agent != decision.request.agent_id.to_string()
        || key != decision.operation_key
        || revision != decision.revision
        || !serde_json::from_str::<SpendRequest>(&request)?.replay_equivalent(&decision.request)
        || serde_json::from_str::<serde_json::Value>(&evaluation)?
            != serde_json::to_value(&decision.evaluation)?
    {
        return Err(invalid(
            "human approval request differs from persisted decision",
        ));
    }
    let current: Option<String> = conn.query_row(
        "SELECT decision FROM spend_authorization_outcomes WHERE agent_id=?1 AND operation_key=?2 AND revision=?3 ORDER BY id DESC LIMIT 1",
        params![agent, key, revision], |row| row.get(0),
    ).optional()?;
    if current.as_deref() != Some("pending_approval") {
        return Err(invalid("human approval request is no longer pending"));
    }
    Ok(())
}

/// Carry the captured evaluated policy, not today's assignment. Legacy pending
/// requests may be confirmed now without inventing their missing old escalation.
fn original_policy(
    conn: &Connection,
    decision: &SpendDecisionRecord,
) -> Result<LifecycleEvidence<LifecyclePolicyEvidence>, StorageError> {
    let input: Option<String> = conn.query_row(
        "SELECT json_extract(payload_json,'$.input') FROM spend_lifecycle_records WHERE owner_user_id=?1 AND json_extract(payload_json,'$.record_type')='event' AND json_extract(payload_json,'$.input.fact.kind')='initial_policy' AND json_extract(payload_json,'$.input.correlation.decision_id')=?2 LIMIT 1",
        params![decision.owner_user_id.to_string(), decision.id.to_string()], |row| row.get(0),
    ).optional()?;
    match input {
        Some(input) => Ok(serde_json::from_str::<LifecycleEventInput>(&input)?
            .explanation
            .policy),
        None => Ok(unavailable(EvidenceUnavailableReason::NotCaptured)),
    }
}
#[allow(clippy::too_many_arguments)]
fn resolution(
    conn: &Connection,
    decision: &SpendDecisionRecord,
    confirmed_by: &UserId,
    resolution: HumanResolution,
    at: DateTime<Utc>,
    policy: LifecycleEvidence<LifecyclePolicyEvidence>,
    budget: LifecycleEvidence<LifecycleBudgetEvidence>,
) -> Result<(), StorageError> {
    activate(conn, decision, at)?;
    append_lifecycle_event_in_transaction(
        conn,
        &event(
            decision,
            LifecycleFact::HumanResolutionConfirmed {
                approval_request_id: decision.id.clone(),
                resolution,
                confirmed_by_user_id: confirmed_by.clone(),
            },
            LifecycleSource::HumanApproval,
            at,
            policy,
            budget,
        ),
    )?;
    Ok(())
}
pub(super) fn save_declined(
    repo: &mut SqliteGovernanceRepository,
    decision: &SpendDecisionRecord,
    confirmed_by: &UserId,
    reasons: &[String],
    at: DateTime<Utc>,
) -> Result<(), StorageError> {
    let tx = repo
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    require_pending(&tx, decision, confirmed_by, at)?;
    let policy = original_policy(&tx, decision)?;
    tx.execute("INSERT INTO spend_authorization_outcomes(agent_id,operation_key,revision,spend_decision_id,decision,reasons_json,created_at) VALUES(?1,?2,?3,?4,'denied',?5,?6)", params![decision.request.agent_id.to_string(),decision.operation_key,decision.revision,decision.id.to_string(),serde_json::to_string(reasons)?,at.to_rfc3339()])?;
    resolution(
        &tx,
        decision,
        confirmed_by,
        HumanResolution::Declined,
        at,
        policy,
        unavailable(EvidenceUnavailableReason::NotApplicable),
    )?;
    tx.commit()?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn save_approved(
    repo: &mut SqliteGovernanceRepository,
    decision: &SpendDecisionRecord,
    confirmed_by: &UserId,
    token: &SpendAuthTokenRecord,
    hold: &BudgetHold,
    balance: &BudgetBalance,
    before: &BudgetWithBalance,
    reasons: &[String],
    at: DateTime<Utc>,
    budget_checked_at: DateTime<Utc>,
) -> Result<(), StorageError> {
    if hold.budget_version_id != before.version.id
        || hold.budget_id != before.budget.id
        || before.budget.agent_id != decision.request.agent_id
        || hold.spend_decision_id != decision.id
        || hold.amount != decision.request.amount
        || token.spend_decision_id != decision.id
        || token.owner_user_id != *confirmed_by
        || !before
            .availability_at(budget_checked_at)
            .is_ok_and(|a| a.allows_reservation())
    {
        return Err(invalid("human grant does not match its request or budget"));
    }
    let tx = repo
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    require_pending(&tx, decision, confirmed_by, at)?;
    validate_before(&tx, before, budget_checked_at)?;
    let policy = original_policy(&tx, decision)?;
    persist_approved_spend_transition(&tx, decision, token, hold, balance, reasons, at)?;
    let budget = budget_evidence(Some(before), &decision.request);
    resolution(
        &tx,
        decision,
        confirmed_by,
        HumanResolution::Approved,
        at,
        policy.clone(),
        budget.clone(),
    )?;
    append_lifecycle_event_in_transaction(
        &tx,
        &event(
            decision,
            LifecycleFact::AuthorizationGranted {
                authorization_id: decision.id.clone(),
            },
            LifecycleSource::HumanApproval,
            at,
            policy,
            budget,
        ),
    )?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
#[path = "human_lifecycle_tests.rs"]
mod tests;
