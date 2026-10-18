//! Hubu-owned financial facts. No provider outcomes are inferred from a claim,
//! HTTP failure, or release. All economic transition facts join their transaction.
use super::*;
use crate::spend::lifecycle::*;
use hubu_common::ids::SpendDecisionId;

fn invalid(message: &str) -> StorageError {
    StorageError::InvalidData(message.into())
}

/// Returns no event for legacy records without a registered owned identity.
/// Their financial transitions remain usable, with absent lifecycle evidence.
fn event(
    conn: &Connection,
    decision_id: &SpendDecisionId,
    source: LifecycleSource,
    fact: LifecycleFact,
    occurred_at: DateTime<Utc>,
) -> Result<Option<LifecycleEventInput>, StorageError> {
    let stored: Option<(String, String, String, u64, String)> = conn.query_row(
        "SELECT owner_user_id,agent_id,operation_key,revision,request_json FROM spend_decisions WHERE id=?1",
        [decision_id.to_string()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
    ).optional()?;
    let Some((owner, agent, operation_key, revision, json)) = stored else {
        return Ok(None);
    };
    let request: SpendRequest = serde_json::from_str(&json)?;
    let identity_tables: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('users','agent_accounts','agent_identities')", [], |r| r.get(0))?;
    if identity_tables != 3 {
        return Ok(None);
    }
    let public_ids: Option<(String, String)> = conn.query_row(
        "SELECT i.pub_id,a.pub_id FROM agent_accounts a JOIN agent_identities i ON i.id=a.agent_id JOIN users u ON u.id=a.owner_user_id WHERE a.id=?1 AND a.agent_id=?2 AND a.owner_user_id=?3 AND i.owner_user_id=?3",
        params![request.agent_account_id.to_string(),agent,owner], |r| Ok((r.get(0)?, r.get(1)?)),
    ).optional()?;
    let registered = public_ids.is_some_and(|(agent, account)| {
        agent.starts_with("agt_")
            && account.starts_with("aga_")
            && super::lifecycle::stable_identifier(&agent)
            && super::lifecycle::stable_identifier(&account)
    });
    if !registered
        || revision == 0
        || revision > i64::MAX as u64
        || operation_key.trim().is_empty()
        || operation_key.len() > 1024
    {
        return Ok(None);
    }
    Ok(Some(LifecycleEventInput {
        schema_version: LIFECYCLE_SCHEMA_VERSION,
        correlation: LifecycleCorrelation {
            owner_user_id: request.owner_user_id.clone(),
            agent_id: request.agent_id.clone(),
            agent_account_id: request.agent_account_id.clone(),
            operation_key,
            attempt_revision: revision,
            decision_id: Some(decision_id.clone()),
            correction_of_revision: revision.checked_sub(1).filter(|r| *r > 0),
        },
        source,
        occurred_at,
        requested_amount: LifecycleEvidence::Known {
            value: LifecycleAmount {
                amount: request.amount,
                currency: request.currency,
            },
        },
        execution_scope: match request.execution_scope {
            Some(scope)
                if [
                    &scope.provider.id,
                    &scope.executor.id,
                    &scope.capability.id,
                    &scope.billing_merchant.id,
                ]
                .iter()
                .all(|id| super::lifecycle::stable_identifier(id)) =>
            {
                LifecycleEvidence::Known {
                    value: LifecycleScopeEvidence {
                        provider: scope.provider.id,
                        executor: scope.executor.id,
                        capability: scope.capability.id,
                        billing_merchant: scope.billing_merchant.id,
                    },
                }
            }
            _ => LifecycleEvidence::Unavailable {
                reason: EvidenceUnavailableReason::NotCaptured,
            },
        },
        fact,
        // Mutable configuration or current balances cannot explain a past fact.
        // Keep a stable payload so polling/restart/late reconciliation dedup.
        explanation: LifecycleExplanation::default(),
    }))
}
fn append(conn: &Connection, input: Option<LifecycleEventInput>) -> Result<(), StorageError> {
    if let Some(input) = input {
        // Activation is when this producer observes evidence, never the backdated
        // lease expiry or a claim reconstructed from preactivation history.
        super::lifecycle::ensure_lifecycle_capture_enabled_in_transaction(
            conn,
            &input.correlation.owner_user_id,
            input.source,
            Utc::now(),
        )?;
        append_lifecycle_event_in_transaction(conn, &input)?;
    }
    Ok(())
}
fn claim_event(
    conn: &Connection,
    claim: &SpendExecutorClaimRecord,
    source: LifecycleSource,
    fact: LifecycleFact,
    at: DateTime<Utc>,
) -> Result<Option<LifecycleEventInput>, StorageError> {
    let token = load_spend_auth_token_by_id(conn, &claim.spend_auth_token_id)?
        .ok_or_else(|| invalid("financial lifecycle claim token is missing"))?;
    event(conn, &token.spend_decision_id, source, fact, at)
}
pub(super) fn capture_claim(
    conn: &Connection,
    claim: &SpendExecutorClaimRecord,
) -> Result<(), StorageError> {
    append(
        conn,
        claim_event(
            conn,
            claim,
            LifecycleSource::Executor,
            LifecycleFact::ExecutionStarted {
                claim_id: claim.id.clone(),
            },
            claim.claimed_at,
        )?,
    )
}

/// This fact means an expired Hubu financial claim, not pre-expiry executor
/// uncertainty. A late observation remains valid after confirmed reconciliation;
/// a claim normally finalized before expiry was never in this backlog.
pub(super) fn validate_reconciliation_entry(
    claim: &SpendExecutorClaimRecord,
    observed_at: DateTime<Utc>,
) -> Result<(), StorageError> {
    let historical = match claim.status {
        SpendExecutorClaimStatus::Claimed => {
            claim.finalized_at.is_none() && claim.reconciled_at.is_none()
        }
        SpendExecutorClaimStatus::Settled | SpendExecutorClaimStatus::Released => {
            claim.finalized_at.is_some_and(|at| at >= claim.expires_at)
                && claim.reconciled_at == claim.finalized_at
                && claim.reconciled_by_user_id.as_ref() == Some(&claim.owner_user_id)
                && claim
                    .reconciliation_evidence
                    .as_ref()
                    .is_some_and(|e| !e.trim().is_empty())
                && claim
                    .provider_reference
                    .as_ref()
                    .is_some_and(|e| !e.trim().is_empty())
        }
    };
    if claim.claimed_at > claim.expires_at || claim.expires_at > observed_at || !historical {
        return Err(invalid(
            "claim was not eligible for financial reconciliation at occurrence",
        ));
    }
    Ok(())
}
fn capture_required(
    conn: &Connection,
    claim: &SpendExecutorClaimRecord,
    now: DateTime<Utc>,
) -> Result<(), StorageError> {
    let input = claim_event(
        conn,
        claim,
        LifecycleSource::Reconciliation,
        LifecycleFact::ReconciliationRequired {
            claim_id: claim.id.clone(),
        },
        claim.expires_at,
    )?;
    if input.is_some() {
        validate_reconciliation_entry(claim, now)?;
    }
    append(conn, input)
}
pub(super) fn capture_finalization(
    conn: &Connection,
    claim: &SpendExecutorClaimRecord,
) -> Result<(), StorageError> {
    let at = claim
        .finalized_at
        .ok_or_else(|| invalid("financial lifecycle finalization time missing"))?;
    let outcome = match claim.status {
        SpendExecutorClaimStatus::Settled => ExecutionFinalization::Settled,
        SpendExecutorClaimStatus::Released => ExecutionFinalization::Released,
        SpendExecutorClaimStatus::Claimed => return Err(invalid("claim is not finalized")),
    };
    let source = if claim.reconciled_at.is_some() {
        LifecycleSource::Reconciliation
    } else {
        LifecycleSource::Executor
    };
    if claim.reconciled_at.is_some() {
        capture_required(conn, claim, at)?;
    }
    append(
        conn,
        claim_event(
            conn,
            claim,
            source,
            LifecycleFact::ExecutionFinalized {
                claim_id: claim.id.clone(),
                outcome,
            },
            at,
        )?,
    )?;
    if let Some(at) = claim.reconciled_at {
        append(
            conn,
            claim_event(
                conn,
                claim,
                LifecycleSource::Reconciliation,
                LifecycleFact::ReconciliationResolved {
                    claim_id: claim.id.clone(),
                    outcome: match outcome {
                        ExecutionFinalization::Settled => ReconciliationResolution::VendorBilled,
                        ExecutionFinalization::Released => {
                            ReconciliationResolution::VendorDidNotBill
                        }
                    },
                },
                at,
            )?,
        )?;
    }
    Ok(())
}
pub(super) fn capture_authorization_release(
    conn: &Connection,
    decision: &SpendDecisionId,
    reason: AuthorizationReleaseReason,
    at: DateTime<Utc>,
) -> Result<(), StorageError> {
    append(
        conn,
        event(
            conn,
            decision,
            LifecycleSource::Executor,
            LifecycleFact::AuthorizationReleased {
                authorization_id: decision.clone(),
                reason,
            },
            at,
        )?,
    )
}
pub(super) fn capture_expired_authorization(
    conn: &Connection,
    hold_id: &str,
    at: DateTime<Utc>,
) -> Result<(), StorageError> {
    // Wallet-used legacy holds can still require ordinary expiry accounting;
    // never label them unused or let unsupported evidence block that accounting.
    let decision: Option<String> = conn.query_row(
        "SELECT h.spend_decision_id FROM budget_holds h JOIN spend_auth_tokens t ON t.spend_decision_id=h.spend_decision_id WHERE h.id=?1 AND h.status='expired' AND h.executor_claim_id IS NULL AND t.used_at IS NULL AND t.used_by_payment_id IS NULL AND NOT EXISTS(SELECT 1 FROM spend_executor_claims cl WHERE cl.spend_auth_token_id=t.id)",
        [hold_id], |r| r.get(0),
    ).optional()?;
    if let Some(decision) = decision {
        capture_authorization_release(
            conn,
            &decision
                .parse()
                .map_err(|_| invalid("invalid expired authorization"))?,
            AuthorizationReleaseReason::Expired,
            at,
        )?;
    }
    Ok(())
}

impl SqliteGovernanceRepository {
    /// Evidence-only observation: no settlement, release, retries, or changes to
    /// claims/holds/balances. This is invoked independently of read requests.
    pub fn observe_financial_lifecycle_at(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<(), StorageError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let has_users: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='users')",
            [],
            |r| r.get(0),
        )?;
        if !has_users {
            tx.commit()?;
            return Ok(());
        }
        // Activate for every known owner even if no fact exists yet. This states
        // producer deployment, not complete historical or provider coverage.
        let owners = {
            let mut stmt = tx.prepare("SELECT id FROM users")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for owner in owners {
            let owner = owner
                .parse()
                .map_err(|_| invalid("invalid financial lifecycle owner"))?;
            for source in [LifecycleSource::Executor, LifecycleSource::Reconciliation] {
                super::lifecycle::ensure_lifecycle_capture_enabled_in_transaction(
                    &tx,
                    &owner,
                    source,
                    Utc::now(),
                )?;
            }
        }
        // Parse timestamps as instants, not lexicographic RFC3339 text: legacy
        // offsets/fraction precision must not shift the expiry boundary.
        let claims = {
            let mut stmt = tx.prepare("SELECT id, spend_auth_token_id, owner_user_id, agent_id, operation_key, lease_profile, status, claimed_at, expires_at, finalized_at, settlement_id, provider_reference, reconciliation_evidence, reconciled_at, reconciled_by_user_id FROM spend_executor_claims WHERE status='claimed'")?;
            let rows = stmt.query_map([], executor_claim_from_row)?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for claim in claims {
            if claim.expires_at <= now && validate_reconciliation_entry(&claim, now).is_ok() {
                capture_required(&tx, &claim, now)?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "financial_lifecycle_tests.rs"]
mod tests;
