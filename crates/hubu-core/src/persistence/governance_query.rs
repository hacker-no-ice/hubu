//! One owner-derived read transaction for list, detail, summary and backlog.
//! Tokens detect filter/metadata mismatches; their plain hash is not a MAC.
//! Every boundary and cursor is independently checked against durable records.
use super::*;
use crate::spend::governance_query::*;
use crate::spend::lifecycle::*;
use hubu_common::ids::AgentAccountId;
use std::collections::{BTreeMap, BTreeSet, HashSet};

impl From<rusqlite::Error> for GovernanceReadError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Unavailable(error.into())
    }
}
impl From<serde_json::Error> for GovernanceReadError {
    fn from(error: serde_json::Error) -> Self {
        Self::Unavailable(error.into())
    }
}

const TOKEN_VERSION: u32 = 1;
const MAX_SNAPSHOT_BYTES: usize = 2048;
const MAX_CURSOR_BYTES: usize = 4096;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Boundary {
    event_id: String,
    recorded_at: DateTime<Utc>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotToken {
    version: u32,
    binding: String,
    watermark: u64,
    boundary: Option<Boundary>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AfterKey {
    occurred_at: DateTime<Utc>,
    sequence: u64,
    event_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CursorToken {
    version: u32,
    snapshot: SnapshotToken,
    after: AfterKey,
}
struct Scope {
    public: GovernanceScope,
    agent: Option<AgentId>,
    account: Option<AgentAccountId>,
}
struct Evaluated {
    summary: GovernanceSummary,
    records: Vec<LifecycleRecord>,
    token: SnapshotToken,
    after: Option<AfterKey>,
}

impl SqliteGovernanceRepository {
    pub fn governance_list(
        &self,
        owner: &UserId,
        query: &GovernanceQuery,
    ) -> Result<GovernancePage, GovernanceReadError> {
        let evaluated = self.evaluate_governance(owner, query)?;
        let start = evaluated
            .after
            .as_ref()
            .map(|key| {
                evaluated
                    .records
                    .iter()
                    .position(|r| after_key(r) == *key)
                    .expect("cursor validated")
                    + 1
            })
            .unwrap_or(0);
        let limit = query.limit.unwrap_or(50);
        let end = start.saturating_add(limit).min(evaluated.records.len());
        let next_cursor = if end < evaluated.records.len() {
            Some(serde_json::to_string(&CursorToken {
                version: TOKEN_VERSION,
                snapshot: evaluated.token,
                after: after_key(&evaluated.records[end - 1]),
            })?)
        } else {
            None
        };
        Ok(GovernancePage {
            facts: evaluated.records[start..end]
                .iter()
                .map(safe_fact)
                .collect(),
            summary: evaluated.summary,
            next_cursor,
        })
    }

    pub fn governance_summary(
        &self,
        owner: &UserId,
        query: &GovernanceQuery,
    ) -> Result<GovernanceSummary, GovernanceReadError> {
        reject_list_arguments(query)?;
        Ok(self.evaluate_governance(owner, query)?.summary)
    }

    /// Unknown, foreign, out-of-filter and post-watermark IDs all return None.
    /// Scope, token and selector validation still run for an absent result.
    pub fn governance_detail(
        &self,
        owner: &UserId,
        event_id: &str,
        query: &GovernanceQuery,
    ) -> Result<Option<GovernanceDetail>, GovernanceReadError> {
        reject_list_arguments(query)?;
        if !event_id.starts_with("lev_") || event_id.len() > 100 {
            return Err(GovernanceReadError::InvalidQuery);
        }
        let evaluated = self.evaluate_governance(owner, query)?;
        Ok(evaluated
            .records
            .iter()
            .find(|r| r.event_id == event_id)
            .map(|record| GovernanceDetail {
                fact: safe_fact(record),
                summary: evaluated.summary.clone(),
            }))
    }

    fn evaluate_governance(
        &self,
        owner: &UserId,
        query: &GovernanceQuery,
    ) -> Result<Evaluated, GovernanceReadError> {
        validate_query(query)?;
        let tx = self.conn.unchecked_transaction()?;
        let scope = resolve_scope(&tx, owner, query)?;
        let binding = filter_binding(owner, &scope.public, query)?;
        let cursor: Option<CursorToken> = query
            .cursor
            .as_deref()
            .map(|raw| decode(raw, MAX_CURSOR_BYTES))
            .transpose()?;
        if cursor.as_ref().is_some_and(|c| {
            c.version != TOKEN_VERSION
                || c.after.sequence > i64::MAX as u64
                || c.after.event_id.len() > 100
        }) {
            return Err(GovernanceReadError::InvalidQuery);
        }
        let supplied: Option<SnapshotToken> = query
            .snapshot
            .as_deref()
            .map(|raw| decode(raw, MAX_SNAPSHOT_BYTES))
            .transpose()?;
        if let (Some(a), Some(b)) = (&supplied, cursor.as_ref().map(|c| &c.snapshot)) {
            if a != b {
                return Err(GovernanceReadError::InvalidQuery);
            }
        }
        let requested = supplied.or_else(|| cursor.as_ref().map(|c| c.snapshot.clone()));
        let token = checked_snapshot(&tx, owner, &binding, requested)?;
        let snapshot = lifecycle::lifecycle_snapshot_from(&tx, owner, Some(token.watermark))?;
        let as_of = token.boundary.as_ref().map(|b| b.recorded_at);
        let scoped: Vec<_> = snapshot
            .events
            .iter()
            .filter(|r| in_scope(r, &scope))
            .collect();
        let mut records: Vec<_> = scoped
            .iter()
            .filter(|r| matches_query(r, query))
            .map(|r| (**r).clone())
            .collect();
        // Parsed UTC nanosecond instants, then durable sequence and random event
        // ID. Neither lexical RFC3339 ordering nor SQLite float dates are used.
        records.sort_by_key(|r| {
            std::cmp::Reverse((r.event.occurred_at, r.sequence, r.event_id.clone()))
        });
        let after = cursor.map(|c| c.after);
        if after
            .as_ref()
            .is_some_and(|key| !records.iter().any(|r| after_key(r) == *key))
        {
            return Err(GovernanceReadError::InvalidQuery);
        }
        let public_snapshot = GovernanceSnapshot {
            token: serde_json::to_string(&token)?,
            watermark: token.watermark,
            captured_evidence_as_of: as_of,
        };
        let summary = summarize(&scope, &snapshot, &scoped, &records, public_snapshot)?;
        tx.commit()?;
        Ok(Evaluated {
            summary,
            records,
            token,
            after,
        })
    }
}

fn unavailable(message: &str) -> GovernanceReadError {
    StorageError::InvalidData(message.into()).into()
}
fn reject_list_arguments(query: &GovernanceQuery) -> Result<(), GovernanceReadError> {
    if query.cursor.is_some() || query.limit.is_some() {
        return Err(GovernanceReadError::InvalidQuery);
    }
    Ok(())
}
fn validate_query(query: &GovernanceQuery) -> Result<(), GovernanceReadError> {
    if query
        .occurred_from
        .zip(query.occurred_before)
        .is_some_and(|(from, before)| from >= before)
    {
        return Err(GovernanceReadError::InvalidQuery);
    }
    if query.limit.is_some_and(|n| n == 0 || n > 200)
        || query.categories.len() > 12
        || query.outcomes.len() > 17
    {
        return Err(GovernanceReadError::InvalidQuery);
    }
    Ok(())
}
fn decode<T: serde::de::DeserializeOwned>(raw: &str, max: usize) -> Result<T, GovernanceReadError> {
    if raw.is_empty() || raw.len() > max {
        return Err(GovernanceReadError::InvalidQuery);
    }
    serde_json::from_str(raw).map_err(|_| GovernanceReadError::InvalidQuery)
}
fn resolve_scope(
    conn: &Connection,
    owner: &UserId,
    query: &GovernanceQuery,
) -> Result<Scope, GovernanceReadError> {
    let known: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id=?1)",
        [owner.to_string()],
        |r| r.get(0),
    )?;
    if !known {
        return Err(GovernanceReadError::InvalidQuery);
    }
    let mut agent = None;
    let mut agent_public = None;
    if let Some(selector) = &query.agent_public_id {
        if !selector.starts_with("agt_") || selector.len() > 100 {
            return Err(GovernanceReadError::InvalidQuery);
        }
        let value: Option<(String, String)> = conn
            .query_row(
                "SELECT id,pub_id FROM agent_identities WHERE owner_user_id=?1 AND pub_id=?2",
                params![owner.to_string(), selector],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (id, public) = value.ok_or(GovernanceReadError::InvalidQuery)?;
        agent = Some(
            id.parse()
                .map_err(|_| unavailable("invalid stored agent"))?,
        );
        agent_public = Some(public);
    }
    let mut account = None;
    let mut account_public = None;
    if let Some(selector) = &query.account_public_id {
        if !selector.starts_with("aga_") || selector.len() > 100 {
            return Err(GovernanceReadError::InvalidQuery);
        }
        let value: Option<(String, String, String, String)> = conn.query_row("SELECT a.id,a.pub_id,i.id,i.pub_id FROM agent_accounts a JOIN agent_identities i ON i.id=a.agent_id WHERE a.owner_user_id=?1 AND i.owner_user_id=?1 AND a.pub_id=?2", params![owner.to_string(), selector], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        let (id, public, agent_id, public_agent) =
            value.ok_or(GovernanceReadError::InvalidQuery)?;
        let implied: AgentId = agent_id
            .parse()
            .map_err(|_| unavailable("invalid stored agent"))?;
        if agent.as_ref().is_some_and(|selected| *selected != implied) {
            return Err(GovernanceReadError::InvalidQuery);
        }
        agent = Some(implied);
        agent_public = Some(public_agent);
        account = Some(
            id.parse()
                .map_err(|_| unavailable("invalid stored account"))?,
        );
        account_public = Some(public);
    }
    Ok(Scope {
        public: GovernanceScope {
            agent_public_id: agent_public,
            account_public_id: account_public,
        },
        agent,
        account,
    })
}
fn filter_binding(
    owner: &UserId,
    scope: &GovernanceScope,
    query: &GovernanceQuery,
) -> Result<String, GovernanceReadError> {
    let categories: BTreeSet<_> = query.categories.iter().collect();
    let outcomes: BTreeSet<_> = query.outcomes.iter().collect();
    let canonical = serde_json::to_vec(&(
        TOKEN_VERSION,
        owner,
        &scope.agent_public_id,
        &scope.account_public_id,
        categories,
        outcomes,
        query.occurred_from,
        query.occurred_before,
    ))?;
    Ok(format!("{:x}", Sha256::digest(canonical)))
}
fn checked_snapshot(
    conn: &Connection,
    owner: &UserId,
    binding: &str,
    supplied: Option<SnapshotToken>,
) -> Result<SnapshotToken, GovernanceReadError> {
    let latest: u64 = conn.query_row(
        "SELECT COALESCE(MAX(sequence),0) FROM spend_lifecycle_records WHERE owner_user_id=?1",
        [owner.to_string()],
        |r| r.get(0),
    )?;
    let watermark = supplied.as_ref().map(|t| t.watermark).unwrap_or(latest);
    if watermark > i64::MAX as u64 || watermark > latest {
        return Err(GovernanceReadError::InvalidQuery);
    }
    let boundary = if watermark == 0 {
        None
    } else {
        let row: Option<(String,String)> = conn.query_row("SELECT event_id,recorded_at FROM spend_lifecycle_records WHERE owner_user_id=?1 AND sequence=?2", params![owner.to_string(), watermark], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        let (event_id, time) = row.ok_or(GovernanceReadError::InvalidQuery)?;
        Some(Boundary {
            event_id,
            recorded_at: parse_timestamp(&time)?,
        })
    };
    let verified = SnapshotToken {
        version: TOKEN_VERSION,
        binding: binding.into(),
        watermark,
        boundary,
    };
    if supplied.is_some_and(|t| t != verified) {
        return Err(GovernanceReadError::InvalidQuery);
    }
    Ok(verified)
}
fn after_key(record: &LifecycleRecord) -> AfterKey {
    AfterKey {
        occurred_at: record.event.occurred_at,
        sequence: record.sequence,
        event_id: record.event_id.clone(),
    }
}
fn in_scope(record: &LifecycleRecord, scope: &Scope) -> bool {
    scope
        .agent
        .as_ref()
        .is_none_or(|a| record.event.correlation.agent_id == *a)
        && scope
            .account
            .as_ref()
            .is_none_or(|a| record.event.correlation.agent_account_id == *a)
}
fn matches_query(record: &LifecycleRecord, query: &GovernanceQuery) -> bool {
    let (category, outcome) = classify(&record.event.fact);
    query
        .occurred_from
        .is_none_or(|from| record.event.occurred_at >= from)
        && query
            .occurred_before
            .is_none_or(|before| record.event.occurred_at < before)
        && (query.categories.is_empty() || query.categories.contains(&category))
        && (query.outcomes.is_empty() || query.outcomes.contains(&outcome))
}
fn safe_fact(record: &LifecycleRecord) -> GovernanceFact {
    let event = &record.event;
    let (category, outcome) = classify(&event.fact);
    GovernanceFact {
        event_public_id: record.event_id.clone(),
        agent_public_id: record.agent_public_id.clone(),
        account_public_id: record.account_public_id.clone(),
        occurred_at: event.occurred_at,
        recorded_at: record.recorded_at,
        attempt_revision: event.correlation.attempt_revision,
        category,
        outcome,
        source: event.source,
        authorization_release_reason: match event.fact {
            LifecycleFact::AuthorizationReleased { reason, .. } => Some(reason),
            _ => None,
        },
        reconciliation_resolution: match event.fact {
            LifecycleFact::ReconciliationResolved { outcome, .. } => Some(outcome),
            _ => None,
        },
        automatic_denial_reason: denial_reason(&event.fact),
        requested_amount: event.requested_amount.clone(),
        execution_scope: event.execution_scope.clone(),
        policy: event.explanation.policy.clone(),
        budget: event.explanation.budget.clone(),
    }
}

type Attempt = (String, String, u64);
fn attempt(record: &LifecycleRecord) -> Attempt {
    let c = &record.event.correlation;
    (
        c.agent_id.to_string(),
        c.operation_key.clone(),
        c.attempt_revision,
    )
}
fn requested_total(
    records: impl Iterator<Item = impl std::borrow::Borrow<LifecycleRecord>>,
) -> Result<GovernanceRequestedTotal, GovernanceReadError> {
    let mut attempts: BTreeMap<Attempt, Option<LifecycleAmount>> = BTreeMap::new();
    for row in records {
        let record = row.borrow();
        let value = attempts.entry(attempt(record)).or_default();
        if let LifecycleEvidence::Known { value: amount } = &record.event.requested_amount {
            if value.as_ref().is_some_and(|old| old != amount) {
                return Err(unavailable("inconsistent captured requested amount"));
            }
            *value = Some(amount.clone());
        }
    }
    let mut total = GovernanceRequestedTotal::default();
    for value in attempts.into_values() {
        if let Some(amount) = value {
            total.known_attempts += 1;
            let index = total
                .per_currency
                .iter()
                .position(|t| t.currency == amount.currency);
            let index = index.unwrap_or_else(|| {
                total.per_currency.push(GovernanceCurrencyTotal {
                    currency: amount.currency,
                    units: 0,
                });
                total.per_currency.len() - 1
            });
            let value = &mut total.per_currency[index].units;
            *value = value
                .checked_add(i128::from(amount.amount.units()))
                .ok_or_else(|| unavailable("governance amount total overflow"))?;
        } else {
            total.unknown_attempts += 1;
        }
    }
    total.per_currency.sort_by_key(|t| t.currency.to_string());
    Ok(total)
}
fn known<T>(evidence: &LifecycleEvidence<T>) -> bool {
    matches!(evidence, LifecycleEvidence::Known { .. })
}
fn summarize(
    scope: &Scope,
    snapshot: &LifecycleSnapshot,
    scoped: &[&LifecycleRecord],
    records: &[LifecycleRecord],
    public_snapshot: GovernanceSnapshot,
) -> Result<GovernanceSummary, GovernanceReadError> {
    let mut attempts = BTreeSet::new();
    let mut operations = BTreeSet::new();
    let mut stages = BTreeMap::new();
    let mut denied = BTreeMap::<_, BTreeSet<Attempt>>::new();
    let mut explanations = BTreeMap::new();
    for record in records {
        let c = &record.event.correlation;
        attempts.insert(attempt(record));
        operations.insert((c.agent_id.to_string(), c.operation_key.clone()));
        let (category, outcome) = classify(&record.event.fact);
        *stages.entry(outcome).or_insert(0) += 1;
        if let Some(reason) = denial_reason(&record.event.fact) {
            denied.entry(reason).or_default().insert(attempt(record));
        }
        let coverage = explanations
            .entry(category)
            .or_insert(GovernanceExplanationCoverage {
                category,
                facts: 0,
                known_requested_amount: 0,
                known_policy: 0,
                known_budget: 0,
                known_execution_scope: 0,
            });
        coverage.facts += 1;
        coverage.known_requested_amount += u64::from(known(&record.event.requested_amount));
        coverage.known_policy += u64::from(known(&record.event.explanation.policy));
        coverage.known_budget += u64::from(known(&record.event.explanation.budget));
        coverage.known_execution_scope += u64::from(known(&record.event.execution_scope));
    }
    let mut opened = HashSet::new();
    let mut closed = HashSet::new();
    for record in scoped.iter().filter(|r| {
        public_snapshot
            .captured_evidence_as_of
            .is_some_and(|as_of| r.event.occurred_at <= as_of)
    }) {
        match record.event.fact.backlog_transition() {
            Some(LifecycleBacklogTransition::Open(subject)) => {
                opened.insert(subject);
            }
            Some(LifecycleBacklogTransition::Resolve(subject)) => {
                closed.insert(subject);
            }
            None => {}
        }
    }
    let mut pending_human_approvals = 0;
    let mut unresolved_reconciliations = 0;
    for subject in opened.difference(&closed) {
        match subject {
            LifecycleBacklogSubject::HumanApproval(_) => pending_human_approvals += 1,
            LifecycleBacklogSubject::Reconciliation(_) => unresolved_reconciliations += 1,
        }
    }
    let backlog = GovernanceBacklog {
        scope: scope.public.clone(),
        captured_evidence_as_of: public_snapshot.captured_evidence_as_of,
        pending_human_approvals,
        unresolved_reconciliations,
    };
    Ok(GovernanceSummary {
        scope: scope.public.clone(),
        snapshot: public_snapshot,
        fact_count: records.len() as u64,
        distinct_attempts: attempts.len() as u64,
        distinct_operations: operations.len() as u64,
        stage_counts_overlap: true,
        stages: stages
            .into_iter()
            .map(|(outcome, fact_count)| GovernanceOutcomeCount {
                outcome,
                fact_count,
            })
            .collect(),
        automatic_denials: denied
            .into_iter()
            .map(|(reason, attempts)| GovernanceDenialCount {
                reason,
                distinct_attempts: attempts.len() as u64,
            })
            .collect(),
        requested: requested_total(records.iter())?,
        automatic_denied_requested: requested_total(
            records
                .iter()
                .filter(|r| denial_reason(&r.event.fact).is_some()),
        )?,
        backlog,
        record_coverage: record_coverage(snapshot),
        unsupported_signals: vec![
            GovernanceUnsupportedSignal::ProviderExecutionFailure,
            GovernanceUnsupportedSignal::PreExpiryBillingUncertainty,
        ],
        explanation_coverage: explanations.into_values().collect(),
        legacy_coverage: legacy_coverage(scope, snapshot),
    })
}
fn record_coverage(snapshot: &LifecycleSnapshot) -> Vec<GovernanceCategoryCoverage> {
    use GovernanceCategory as C;
    use LifecycleSource as S;
    let mappings: &[(C, &[S])] = &[
        (C::InitialPolicy, &[S::PolicyEngine]),
        (C::AutomatedAdmission, &[S::AutomatedAdmission]),
        (C::AdmissionFailure, &[S::AutomatedAdmission]),
        (C::HumanEscalation, &[S::HumanApproval]),
        (C::HumanResolution, &[S::HumanApproval]),
        (C::Authorization, &[S::AutomatedAdmission, S::HumanApproval]),
        (C::AuthorizationRelease, &[S::Executor]),
        (C::ExecutionStart, &[S::Executor]),
        (C::ExecutionFailure, &[]),
        (C::ExecutionFinalization, &[S::Executor, S::Reconciliation]),
        (C::ReconciliationEntry, &[S::Reconciliation]),
        (C::ReconciliationResolution, &[S::Reconciliation]),
    ];
    mappings
        .iter()
        .map(|(category, sources)| GovernanceCategoryCoverage {
            category: *category,
            supported: !sources.is_empty(),
            complete_history: false,
            capture_declarations: snapshot
                .capture_markers
                .iter()
                .filter(|m| sources.contains(&m.source))
                .map(|m| GovernanceCaptureDeclaration {
                    source: m.source,
                    state: m.state,
                    effective_at: m.effective_at,
                    recorded_at: m.recorded_at,
                })
                .collect(),
        })
        .collect()
}
fn legacy_coverage(scope: &Scope, snapshot: &LifecycleSnapshot) -> GovernanceLegacyCoverage {
    let mut result = GovernanceLegacyCoverage {
        observed_at: None,
        assessed: false,
        unlinked_metrics: vec![],
        unlinked_human_approval_backlog: false,
        unlinked_reconciliation_backlog: false,
        broader_attribution_unknown: false,
    };
    let Some(inventory) = snapshot
        .coverage_inventories
        .iter()
        .max_by_key(|i| i.sequence)
    else {
        return result;
    };
    result.observed_at = Some(inventory.observed_at);
    let mut metrics = BTreeSet::new();
    for observation in &inventory.scopes {
        use LifecycleCoverageScope as S;
        let (included, assessment, broader) =
            match (&observation.scope, &scope.agent, &scope.account) {
                (S::Account { account_id, .. }, _, Some(selected)) => {
                    (account_id == selected, account_id == selected, false)
                }
                (S::Account { agent_id, .. }, Some(selected), None) => {
                    (agent_id == selected, false, false)
                }
                (S::AgentUnattributed { agent_id }, Some(selected), None) => {
                    (agent_id == selected, agent_id == selected, false)
                }
                (S::AgentUnattributed { agent_id }, Some(selected), Some(_)) => {
                    (false, false, agent_id == selected)
                }
                (S::OwnerUnattributed, None, None) => (true, true, false),
                (S::OwnerUnattributed, _, _) => (false, false, true),
                (_, None, None) => (true, false, false),
                _ => (false, false, false),
            };
        result.assessed |= assessment;
        if broader && (!observation.gaps.is_empty() || !observation.backlog_gaps.is_empty()) {
            result.broader_attribution_unknown = true;
        }
        if included {
            metrics.extend(observation.gaps.iter().map(|g| g.metric));
            result.unlinked_human_approval_backlog |= observation
                .backlog_gaps
                .contains(&LifecycleCoverageBacklog::HumanApproval);
            result.unlinked_reconciliation_backlog |= observation
                .backlog_gaps
                .contains(&LifecycleCoverageBacklog::FinancialReconciliation);
        }
    }
    result.unlinked_metrics = metrics.into_iter().collect();
    result
}

#[cfg(test)]
#[path = "governance_query_tests.rs"]
mod tests;
