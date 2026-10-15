//! Aggregate, append-only observations of canonical evidence not represented by
//! enumerable lifecycle facts. Collection is independent of query requests.
use super::*;
use crate::spend::lifecycle::*;
use std::collections::BTreeSet;

const COLLECTOR_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum StoredScope {
    Account {
        agent_id: AgentId,
        account_id: hubu_common::ids::AgentAccountId,
    },
    AgentUnattributed {
        agent_id: AgentId,
    },
    OwnerUnattributed,
}
impl StoredScope {
    fn key(&self) -> String {
        match self {
            Self::Account { account_id, .. } => format!("account:{account_id}"),
            Self::AgentUnattributed { agent_id } => format!("agent:{agent_id}"),
            Self::OwnerUnattributed => "owner".into(),
        }
    }
    fn into_scope(self) -> LifecycleCoverageScope {
        match self {
            Self::Account {
                agent_id,
                account_id,
            } => LifecycleCoverageScope::Account {
                agent_id,
                account_id,
            },
            Self::AgentUnattributed { agent_id } => {
                LifecycleCoverageScope::AgentUnattributed { agent_id }
            }
            Self::OwnerUnattributed => LifecycleCoverageScope::OwnerUnattributed,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredScopeObservation {
    scope: StoredScope,
    gaps: Vec<LifecycleCoverageGap>,
    backlog_gaps: Vec<LifecycleCoverageBacklog>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoredCoverageInventory {
    collector_version: u32,
    observed_at: DateTime<Utc>,
    scopes: Vec<StoredScopeObservation>,
}
impl StoredCoverageInventory {
    pub(super) fn into_snapshot(
        self,
        sequence: u64,
        recorded_at: DateTime<Utc>,
    ) -> Result<LifecycleCoverageInventory, StorageError> {
        if self.collector_version != COLLECTOR_VERSION {
            return Err(invalid("unsupported lifecycle coverage collector"));
        }
        Ok(LifecycleCoverageInventory {
            sequence,
            observed_at: self.observed_at,
            recorded_at,
            collector_version: self.collector_version,
            scopes: self
                .scopes
                .into_iter()
                .map(|s| LifecycleCoverageScopeObservation {
                    scope: s.scope.into_scope(),
                    gaps: s.gaps,
                    backlog_gaps: s.backlog_gaps,
                })
                .collect(),
        })
    }
}
fn invalid(message: &str) -> StorageError {
    StorageError::InvalidData(message.into())
}
fn identity_catalog_exists(conn: &Connection) -> Result<bool, StorageError> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='agent_identities') AND EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='agent_accounts')", [], |r|r.get(0))?)
}

// Owner, agent and account provenance must agree. A missing/mismatched account
// falls back only to a proven owned agent; ambiguous agent ownership stays in
// the owner-only bucket. No canonical IDs are published by this metadata.
const DECISIONS: &str = "WITH scoped_decisions AS (
 SELECT d.*, CASE
 WHEN i.id IS NOT NULL AND json_extract(d.request_json,'$.owner_user_id')=d.owner_user_id
  AND json_extract(d.request_json,'$.agent_id')=d.agent_id AND a.id IS NOT NULL THEN 'account:'||a.id
 WHEN i.id IS NOT NULL AND json_extract(d.request_json,'$.owner_user_id')=d.owner_user_id
  AND json_extract(d.request_json,'$.agent_id')=d.agent_id THEN 'agent:'||d.agent_id
 ELSE 'owner' END AS inventory_scope
 FROM spend_decisions d
 LEFT JOIN agent_identities i ON i.id=d.agent_id AND i.owner_user_id=d.owner_user_id
 LEFT JOIN agent_accounts a ON a.id=json_extract(d.request_json,'$.agent_account_id')
  AND a.agent_id=d.agent_id AND a.owner_user_id=d.owner_user_id
 WHERE d.owner_user_id=?1
)";
fn missing_event(decision: &str, kind: &str) -> String {
    format!("NOT EXISTS(SELECT 1 FROM spend_lifecycle_records e WHERE e.owner_user_id=?1 AND json_extract(e.payload_json,'$.record_type')='event' AND json_extract(e.payload_json,'$.input.correlation.decision_id')={decision} AND json_extract(e.payload_json,'$.input.fact.kind')='{kind}')")
}
fn latest_outcome(condition: &str) -> String {
    format!("EXISTS(SELECT 1 FROM spend_authorization_outcomes o WHERE o.id=(SELECT MAX(latest.id) FROM spend_authorization_outcomes latest WHERE latest.agent_id=d.agent_id AND latest.operation_key=d.operation_key AND latest.revision=d.revision) AND {condition})")
}
fn decision_probe(
    conn: &Connection,
    owner: &UserId,
    scope: &StoredScope,
    condition: &str,
    kind: &str,
) -> Result<bool, StorageError> {
    Ok(conn.query_row(&format!("{DECISIONS} SELECT EXISTS(SELECT 1 FROM scoped_decisions d WHERE inventory_scope=?2 AND ({condition}) AND {} LIMIT 1)", missing_event("d.id",kind)), params![owner.to_string(),scope.key()], |r|r.get(0))?)
}
fn collect_decision_gaps(
    conn: &Connection,
    owner: &UserId,
    scope: &StoredScope,
    gaps: &mut BTreeSet<(LifecycleSource, LifecycleCount)>,
    backlogs: &mut BTreeSet<LifecycleCoverageBacklog>,
) -> Result<(), StorageError> {
    let effect = |value: &str| format!("json_extract(d.evaluation_json,'$.decision')='{value}'");
    for (effect_name, metric) in [
        ("allow", LifecycleCount::PolicyAllow),
        ("deny", LifecycleCount::PolicyDeny),
        ("needs_approval", LifecycleCount::PolicyNeedsApproval),
    ] {
        if decision_probe(conn, owner, scope, &effect(effect_name), "initial_policy")? {
            gaps.insert((LifecycleSource::PolicyEngine, metric));
        }
    }
    for (policy, outcome, metric) in [
        ("allow", "allowed", LifecycleCount::AdmissionAllowed),
        ("allow", "denied", LifecycleCount::AdmissionDenied),
        ("deny", "denied", LifecycleCount::AdmissionDenied),
    ] {
        let condition = format!(
            "{} AND {}",
            effect(policy),
            latest_outcome(&format!(
                "o.decision='{outcome}' AND o.spend_decision_id=d.id"
            ))
        );
        if decision_probe(conn, owner, scope, &condition, "automated_admission")? {
            gaps.insert((LifecycleSource::AutomatedAdmission, metric));
        }
    }
    let human = effect("needs_approval");
    if decision_probe(conn, owner, scope, &human, "human_escalation")? {
        gaps.insert((
            LifecycleSource::HumanApproval,
            LifecycleCount::HumanEscalation,
        ));
    }
    let pending = format!(
        "{human} AND {}",
        latest_outcome("o.decision='pending_approval'")
    );
    if decision_probe(conn, owner, scope, &pending, "human_escalation")? {
        backlogs.insert(LifecycleCoverageBacklog::HumanApproval);
    }
    for (outcome, metric) in [
        ("allowed", LifecycleCount::HumanApproved),
        ("denied", LifecycleCount::HumanDeclined),
    ] {
        let condition = format!(
            "{human} AND {}",
            latest_outcome(&format!(
                "o.decision='{outcome}' AND o.spend_decision_id=d.id"
            ))
        );
        if decision_probe(conn, owner, scope, &condition, "human_resolution_confirmed")? {
            gaps.insert((LifecycleSource::HumanApproval, metric));
            // A missing closure can also leave a false open in the event fold.
            backlogs.insert(LifecycleCoverageBacklog::HumanApproval);
        }
    }
    let grant = format!("{} AND EXISTS(SELECT 1 FROM spend_auth_tokens t JOIN budget_holds h ON h.spend_decision_id=t.spend_decision_id WHERE t.spend_decision_id=d.id AND t.owner_user_id=d.owner_user_id)",latest_outcome("o.decision='allowed' AND o.spend_decision_id=d.id"));
    for (policy, source) in [
        ("allow", LifecycleSource::AutomatedAdmission),
        ("needs_approval", LifecycleSource::HumanApproval),
    ] {
        if decision_probe(
            conn,
            owner,
            scope,
            &format!("{} AND {grant}", effect(policy)),
            "authorization_granted",
        )? {
            gaps.insert((source, LifecycleCount::AuthorizationGranted));
        }
    }
    let release = "EXISTS(SELECT 1 FROM spend_auth_tokens t JOIN budget_holds h ON h.spend_decision_id=t.spend_decision_id LEFT JOIN spend_authorization_voids v ON v.spend_auth_token_id=t.id WHERE t.spend_decision_id=d.id AND t.owner_user_id=d.owner_user_id AND h.executor_claim_id IS NULL AND t.used_at IS NULL AND t.used_by_payment_id IS NULL AND NOT EXISTS(SELECT 1 FROM spend_executor_claims c WHERE c.spend_auth_token_id=t.id) AND (h.status='expired' OR (h.status='released' AND t.revoked_at IS NOT NULL AND v.reason IN ('cancelled','executor_rejected_before_claim'))))";
    if decision_probe(conn, owner, scope, release, "authorization_released")? {
        gaps.insert((
            LifecycleSource::Executor,
            LifecycleCount::AuthorizationReleased,
        ));
    }
    Ok(())
}

const CLAIMS: &str = "WITH scoped_claims AS (
 SELECT cl.*,d.id AS decision_id,CASE
 WHEN i.id IS NOT NULL AND d.id IS NOT NULL AND json_extract(d.request_json,'$.owner_user_id')=cl.owner_user_id
  AND json_extract(d.request_json,'$.agent_id')=cl.agent_id AND a.id IS NOT NULL THEN 'account:'||a.id
 WHEN i.id IS NOT NULL AND (d.id IS NULL OR (json_extract(d.request_json,'$.owner_user_id')=cl.owner_user_id
  AND json_extract(d.request_json,'$.agent_id')=cl.agent_id)) THEN 'agent:'||cl.agent_id
 ELSE 'owner' END AS inventory_scope
 FROM spend_executor_claims cl
 LEFT JOIN spend_auth_tokens t ON t.id=cl.spend_auth_token_id AND t.owner_user_id=cl.owner_user_id
 LEFT JOIN spend_decisions d ON d.id=t.spend_decision_id AND d.owner_user_id=cl.owner_user_id
  AND d.agent_id=cl.agent_id AND d.operation_key=cl.operation_key
 LEFT JOIN agent_identities i ON i.id=cl.agent_id AND i.owner_user_id=cl.owner_user_id
 LEFT JOIN agent_accounts a ON a.id=json_extract(d.request_json,'$.agent_account_id')
  AND a.agent_id=cl.agent_id AND a.owner_user_id=cl.owner_user_id
 WHERE cl.owner_user_id=?1
)";
fn missing_claim_event(kind: &str) -> String {
    format!("NOT EXISTS(SELECT 1 FROM spend_lifecycle_records e WHERE e.owner_user_id=?1 AND json_extract(e.payload_json,'$.record_type')='event' AND json_extract(e.payload_json,'$.input.correlation.decision_id')=cl.decision_id AND json_extract(e.payload_json,'$.input.fact.kind')='{kind}' AND json_extract(e.payload_json,'$.input.fact.claim_id')=cl.id)")
}
fn collect_claim_gaps(
    conn: &Connection,
    owner: &UserId,
    scope: &StoredScope,
    at: DateTime<Utc>,
    gaps: &mut BTreeSet<(LifecycleSource, LifecycleCount)>,
    backlogs: &mut BTreeSet<LifecycleCoverageBacklog>,
) -> Result<(), StorageError> {
    let sql=format!("{CLAIMS} SELECT id,spend_auth_token_id,owner_user_id,agent_id,operation_key,lease_profile,status,claimed_at,expires_at,finalized_at,settlement_id,provider_reference,reconciliation_evidence,reconciled_at,reconciled_by_user_id,decision_id FROM scoped_claims cl WHERE inventory_scope=?2 AND ({} OR ((finalized_at IS NOT NULL) AND {}) OR ((status='claimed' OR reconciled_at IS NOT NULL) AND {}) OR (reconciled_at IS NOT NULL AND {}))",missing_claim_event("execution_started"),missing_claim_event("execution_finalized"),missing_claim_event("reconciliation_required"),missing_claim_event("reconciliation_resolved"));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![owner.to_string(), scope.key()], |row| {
        Ok((
            executor_claim_from_row(row)?,
            row.get::<_, Option<String>>(15)?,
        ))
    })?;
    for row in rows {
        let (claim, decision) = row?;
        let has = |kind: &str| -> Result<bool, StorageError> {
            Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM spend_lifecycle_records WHERE owner_user_id=?1 AND json_extract(payload_json,'$.record_type')='event' AND json_extract(payload_json,'$.input.correlation.decision_id')=?2 AND json_extract(payload_json,'$.input.fact.kind')=?3 AND json_extract(payload_json,'$.input.fact.claim_id')=?4 LIMIT 1)",params![owner.to_string(),decision,kind,claim.id.to_string()],|r|r.get(0))?)
        };
        if !has("execution_started")? {
            gaps.insert((LifecycleSource::Executor, LifecycleCount::ExecutionStarted));
        }
        let finalized = claim.finalized_at.is_some()
            && matches!(
                claim.status,
                SpendExecutorClaimStatus::Settled | SpendExecutorClaimStatus::Released
            );
        if finalized && !has("execution_finalized")? {
            let source = if claim.reconciled_at.is_some() {
                LifecycleSource::Reconciliation
            } else {
                LifecycleSource::Executor
            };
            let metric = if claim.status == SpendExecutorClaimStatus::Settled {
                LifecycleCount::ExecutionSettled
            } else {
                LifecycleCount::ExecutionReleased
            };
            gaps.insert((source, metric));
        }
        // Reuse the actual financial producer's historical eligibility rules;
        // never interpret arbitrary release/error as failed execution or debt.
        if super::financial_lifecycle::validate_reconciliation_entry(&claim, at).is_ok() {
            if !has("reconciliation_required")? {
                gaps.insert((
                    LifecycleSource::Reconciliation,
                    LifecycleCount::ReconciliationRequired,
                ));
                backlogs.insert(LifecycleCoverageBacklog::FinancialReconciliation);
            }
            if claim.reconciled_at.is_some() && !has("reconciliation_resolved")? {
                gaps.insert((
                    LifecycleSource::Reconciliation,
                    LifecycleCount::ReconciliationResolved,
                ));
                backlogs.insert(LifecycleCoverageBacklog::FinancialReconciliation);
            }
        }
    }
    Ok(())
}

impl SqliteGovernanceRepository {
    /// Collect all known owners independently of read requests. Each owner's
    /// canonical inventory and append share one write transaction. Metadata is
    /// proportional to scopes/metric kinds, never to historical row count.
    pub fn observe_all_lifecycle_coverage_inventories_at(
        &mut self,
        at: DateTime<Utc>,
    ) -> Result<(), StorageError> {
        let has_users: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='users')",
            [],
            |r| r.get(0),
        )?;
        if !has_users || !identity_catalog_exists(&self.conn)? {
            return Ok(());
        }
        let owners = {
            let mut stmt = self.conn.prepare("SELECT id FROM users ORDER BY id")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for owner in owners {
            self.observe_lifecycle_coverage_inventory_at(
                &owner
                    .parse()
                    .map_err(|_| invalid("invalid inventory owner"))?,
                at,
            )?;
        }
        Ok(())
    }
    /// First observed-empty state appends; unchanged scans do not move history.
    /// Scan work may grow when proving absence in large canonical histories.
    pub fn observe_lifecycle_coverage_inventory_at(
        &mut self,
        owner: &UserId,
        at: DateTime<Utc>,
    ) -> Result<Option<u64>, StorageError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !identity_catalog_exists(&tx)? {
            return Err(invalid("lifecycle inventory identity catalog unavailable"));
        }
        let known: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM users WHERE id=?1)",
            [owner.to_string()],
            |r| r.get(0),
        )?;
        if !known {
            return Err(invalid("unknown lifecycle inventory owner"));
        }
        let mut scopes = vec![StoredScope::OwnerUnattributed];
        {
            let mut stmt =
                tx.prepare("SELECT id FROM agent_identities WHERE owner_user_id=?1 ORDER BY id")?;
            let agents = stmt.query_map([owner.to_string()], |r| r.get::<_, String>(0))?;
            for agent in agents {
                scopes.push(StoredScope::AgentUnattributed {
                    agent_id: agent?
                        .parse()
                        .map_err(|_| invalid("invalid inventory agent"))?,
                });
            }
        }
        {
            let mut stmt=tx.prepare("SELECT a.agent_id,a.id FROM agent_accounts a JOIN agent_identities i ON i.id=a.agent_id AND i.owner_user_id=a.owner_user_id WHERE a.owner_user_id=?1 ORDER BY a.agent_id,a.id")?;
            let accounts = stmt.query_map([owner.to_string()], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            for account in accounts {
                let (agent, account) = account?;
                scopes.push(StoredScope::Account {
                    agent_id: agent
                        .parse()
                        .map_err(|_| invalid("invalid inventory agent"))?,
                    account_id: account
                        .parse()
                        .map_err(|_| invalid("invalid inventory account"))?,
                });
            }
        }
        let mut observations = Vec::new();
        for scope in scopes {
            let mut gaps = BTreeSet::new();
            let mut backlog_gaps = BTreeSet::new();
            collect_decision_gaps(&tx, owner, &scope, &mut gaps, &mut backlog_gaps)?;
            collect_claim_gaps(&tx, owner, &scope, at, &mut gaps, &mut backlog_gaps)?;
            observations.push(StoredScopeObservation {
                scope,
                gaps: gaps
                    .into_iter()
                    .map(|(source, metric)| LifecycleCoverageGap { source, metric })
                    .collect(),
                backlog_gaps: backlog_gaps.into_iter().collect(),
            });
        }
        let inventory = StoredCoverageInventory {
            collector_version: COLLECTOR_VERSION,
            observed_at: at,
            scopes: observations,
        };
        let latest:Option<(u64,String)>=tx.query_row("SELECT sequence,json_extract(payload_json,'$.inventory') FROM spend_lifecycle_records WHERE owner_user_id=?1 AND json_extract(payload_json,'$.record_type')='coverage_inventory' ORDER BY sequence DESC LIMIT 1",[owner.to_string()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some((_, json)) = &latest {
            let previous: StoredCoverageInventory = serde_json::from_str(json)?;
            if previous.collector_version == inventory.collector_version
                && previous.scopes == inventory.scopes
            {
                tx.commit()?;
                return Ok(None);
            }
        }
        let predecessor = latest.as_ref().map(|(sequence, _)| *sequence).unwrap_or(0);
        // The predecessor distinguishes a returning state (A -> B -> A), while
        // the write lock prevents duplicate parallel observations.
        let identity = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(
                owner,
                "coverage_inventory",
                COLLECTOR_VERSION,
                predecessor,
                &inventory
            ))?)
        );
        let (sequence, _, _, _) = super::lifecycle::append_payload(
            &tx,
            owner,
            &identity,
            &super::lifecycle::StoredLifecyclePayload::CoverageInventory {
                schema_version: LIFECYCLE_SCHEMA_VERSION,
                inventory,
            },
        )?;
        tx.commit()?;
        Ok(Some(sequence))
    }
}

#[cfg(test)]
#[path = "lifecycle_coverage_tests.rs"]
mod tests;
