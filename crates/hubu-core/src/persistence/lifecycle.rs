//! Additive internal evidence store. No producers are enabled by this module.
use super::*;
use crate::spend::lifecycle::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendLifecycleResult {
    pub record: LifecycleRecord,
    pub idempotent_replay: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "record_type", rename_all = "snake_case", deny_unknown_fields)]
enum StoredLifecyclePayload {
    Event {
        input: Box<LifecycleEventInput>,
        agent_public_id: String,
        account_public_id: String,
    },
    Capture {
        schema_version: u32,
        source: LifecycleSource,
        state: LifecycleCaptureState,
        effective_at: DateTime<Utc>,
    },
}

impl SqliteGovernanceRepository {
    pub(super) fn init_lifecycle_schema(&mut self) -> Result<(), StorageError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS spend_lifecycle_schema_versions (
                version INTEGER PRIMARY KEY CHECK(version > 0), installed_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS spend_lifecycle_records (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                owner_user_id TEXT NOT NULL,
                semantic_identity TEXT NOT NULL UNIQUE,
                event_id TEXT NOT NULL UNIQUE,
                payload_json TEXT NOT NULL,
                recorded_at TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS spend_lifecycle_owner_sequence
                ON spend_lifecycle_records(owner_user_id, sequence);
             CREATE TRIGGER IF NOT EXISTS spend_lifecycle_records_no_update
                BEFORE UPDATE ON spend_lifecycle_records BEGIN
                SELECT RAISE(ABORT, 'lifecycle evidence is immutable'); END;
             CREATE TRIGGER IF NOT EXISTS spend_lifecycle_records_no_delete
                BEFORE DELETE ON spend_lifecycle_records BEGIN
                SELECT RAISE(ABORT, 'lifecycle evidence is immutable'); END;
             CREATE TRIGGER IF NOT EXISTS spend_lifecycle_schema_no_update
                BEFORE UPDATE ON spend_lifecycle_schema_versions BEGIN
                SELECT RAISE(ABORT, 'lifecycle schema history is immutable'); END;
             CREATE TRIGGER IF NOT EXISTS spend_lifecycle_schema_no_delete
                BEFORE DELETE ON spend_lifecycle_schema_versions BEGIN
                SELECT RAISE(ABORT, 'lifecycle schema history is immutable'); END;",
        )?;
        self.conn.execute("INSERT OR IGNORE INTO spend_lifecycle_schema_versions(version,installed_at) VALUES(?1,?2)",
            params![LIFECYCLE_SCHEMA_VERSION, Utc::now().to_rfc3339()])?;
        Ok(())
    }

    /// Convenience transaction for isolated writers. Stateful transitions should
    /// use append_lifecycle_event_in_transaction inside their existing write.
    pub fn append_lifecycle_event(
        &mut self,
        input: &LifecycleEventInput,
    ) -> Result<AppendLifecycleResult, StorageError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = append_lifecycle_event_in_transaction(&tx, input)?;
        tx.commit()?;
        Ok(result)
    }

    /// Explicit owner/source capture declaration. No declaration is created by
    /// schema installation, and a declaration does not backfill any facts.
    pub fn append_lifecycle_capture_marker(
        &mut self,
        owner: &UserId,
        source: LifecycleSource,
        state: LifecycleCaptureState,
        effective_at: DateTime<Utc>,
    ) -> Result<LifecycleCaptureMarker, StorageError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let known: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM users WHERE id=?1)",
            [owner.to_string()],
            |row| row.get(0),
        )?;
        if !known {
            return Err(invalid("unknown lifecycle owner"));
        }
        let payload = StoredLifecyclePayload::Capture {
            schema_version: LIFECYCLE_SCHEMA_VERSION,
            source,
            state,
            effective_at,
        };
        let identity = identity_hash(&(
            owner,
            "capture",
            LIFECYCLE_SCHEMA_VERSION,
            source,
            effective_at,
        ))?;
        let (sequence, _, recorded_at, _) = append_payload(&tx, owner, &identity, &payload)?;
        tx.commit()?;
        Ok(LifecycleCaptureMarker {
            sequence,
            schema_version: LIFECYCLE_SCHEMA_VERSION,
            source,
            state,
            effective_at,
            recorded_at,
        })
    }

    /// A consistent owner snapshot, optionally frozen at an earlier recorded
    /// watermark. No occurred-time filter: that would lose pre-period backlog.
    /// Consumers must count by occurred_at and resolve backlog subjects with
    /// terminal resolution dominating late recorded opens.
    pub fn lifecycle_snapshot(
        &self,
        owner: &UserId,
        through: Option<u64>,
    ) -> Result<LifecycleSnapshot, StorageError> {
        let tx = self.conn.unchecked_transaction()?;
        let latest: u64 = tx.query_row(
            "SELECT COALESCE(MAX(sequence),0) FROM spend_lifecycle_records WHERE owner_user_id=?1",
            [owner.to_string()],
            |row| row.get(0),
        )?;
        let watermark = through.unwrap_or(latest);
        if watermark > latest {
            return Err(invalid("lifecycle watermark is ahead of owner history"));
        }
        let mut events = Vec::new();
        let mut capture_markers = Vec::new();
        let mut stmt = tx.prepare("SELECT sequence,event_id,payload_json,recorded_at FROM spend_lifecycle_records WHERE owner_user_id=?1 AND sequence<=?2 ORDER BY sequence")?;
        let rows = stmt.query_map(params![owner.to_string(), watermark], |row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (sequence, event_id, payload_json, recorded_at) = row?;
            let recorded_at = parse_timestamp(&recorded_at)?;
            match serde_json::from_str::<StoredLifecyclePayload>(&payload_json)? {
                StoredLifecyclePayload::Event {
                    input,
                    agent_public_id,
                    account_public_id,
                } => {
                    if input.correlation.owner_user_id != *owner
                        || input.schema_version != LIFECYCLE_SCHEMA_VERSION
                    {
                        return Err(invalid("invalid stored lifecycle scope or version"));
                    }
                    events.push(LifecycleRecord {
                        sequence,
                        event_id,
                        agent_public_id,
                        account_public_id,
                        recorded_at,
                        event: *input,
                    });
                }
                StoredLifecyclePayload::Capture {
                    schema_version,
                    source,
                    state,
                    effective_at,
                } => {
                    if schema_version != LIFECYCLE_SCHEMA_VERSION {
                        return Err(invalid("unsupported lifecycle capture version"));
                    }
                    capture_markers.push(LifecycleCaptureMarker {
                        sequence,
                        schema_version,
                        source,
                        state,
                        effective_at,
                        recorded_at,
                    });
                }
            }
        }
        drop(stmt);
        tx.commit()?;
        Ok(LifecycleSnapshot {
            schema_version: LIFECYCLE_SCHEMA_VERSION,
            watermark,
            events,
            capture_markers,
        })
    }
}

/// Joins a caller-owned SQLite transaction; never commits or opens a nested
/// transaction. The caller must roll back its complete write on any error.
/// Public event identity is random; semantic identity is private and independent
/// of arrival/recording time. Conflicting retries fail without exposing payload.
pub fn append_lifecycle_event_in_transaction(
    conn: &Connection,
    input: &LifecycleEventInput,
) -> Result<AppendLifecycleResult, StorageError> {
    if conn.is_autocommit() {
        return Err(invalid(
            "lifecycle append requires a caller-owned transaction",
        ));
    }
    let (agent_public_id, account_public_id) = validate_input(conn, input)?;
    let c = &input.correlation;
    let identity = identity_hash(&(
        &c.owner_user_id,
        &c.agent_id,
        &c.operation_key,
        c.attempt_revision,
        input.fact.slot(),
    ))?;
    let payload = StoredLifecyclePayload::Event {
        input: Box::new(input.clone()),
        agent_public_id: agent_public_id.clone(),
        account_public_id: account_public_id.clone(),
    };
    let (sequence, event_id, recorded_at, idempotent_replay) =
        append_payload(conn, &c.owner_user_id, &identity, &payload)?;
    Ok(AppendLifecycleResult {
        record: LifecycleRecord {
            sequence,
            event_id,
            agent_public_id,
            account_public_id,
            recorded_at,
            event: input.clone(),
        },
        idempotent_replay,
    })
}

fn invalid(message: &str) -> StorageError {
    StorageError::InvalidData(message.into())
}
fn identity_hash(value: &impl Serialize) -> Result<String, StorageError> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
fn append_payload(
    conn: &Connection,
    owner: &UserId,
    identity: &str,
    payload: &StoredLifecyclePayload,
) -> Result<(u64, String, DateTime<Utc>, bool), StorageError> {
    let json = serde_json::to_string(payload)?;
    let existing:Option<(u64,String,String,String)> = conn.query_row("SELECT sequence,event_id,recorded_at,payload_json FROM spend_lifecycle_records WHERE semantic_identity=?1 AND owner_user_id=?2",params![identity,owner.to_string()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional()?;
    if let Some((sequence, event_id, recorded_at, stored)) = existing {
        if serde_json::from_str::<StoredLifecyclePayload>(&stored)? != *payload {
            return Err(invalid("conflicting lifecycle event replay"));
        }
        return Ok((sequence, event_id, parse_timestamp(&recorded_at)?, true));
    }
    let event_id = format!("lev_{}", hubu_common::ids::SpendDecisionId::new());
    let recorded_at = Utc::now();
    conn.execute("INSERT INTO spend_lifecycle_records(owner_user_id,semantic_identity,event_id,payload_json,recorded_at) VALUES(?1,?2,?3,?4,?5)",params![owner.to_string(),identity,event_id,json,recorded_at.to_rfc3339()])?;
    let sequence = u64::try_from(conn.last_insert_rowid())
        .map_err(|_| invalid("invalid lifecycle sequence"))?;
    Ok((sequence, event_id, recorded_at, false))
}

fn validate_input(
    conn: &Connection,
    input: &LifecycleEventInput,
) -> Result<(String, String), StorageError> {
    let c = &input.correlation;
    if input.schema_version != LIFECYCLE_SCHEMA_VERSION
        || c.attempt_revision == 0
        || c.attempt_revision > i64::MAX as u64
        || c.operation_key.trim().is_empty()
        || c.operation_key.len() > 1024
        || c.correction_of_revision
            .is_some_and(|r| r == 0 || r >= c.attempt_revision)
        || !input.fact.accepts_source(input.source)
    {
        return Err(invalid("invalid lifecycle version, correlation, or source"));
    }
    let public_ids:Option<(String,String)> = conn.query_row("SELECT i.pub_id,a.pub_id FROM agent_accounts a JOIN agent_identities i ON i.id=a.agent_id WHERE a.id=?1 AND a.agent_id=?2 AND a.owner_user_id=?3 AND i.owner_user_id=?3",params![c.agent_account_id.to_string(),c.agent_id.to_string(),c.owner_user_id.to_string()],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
    let public_ids = public_ids.ok_or_else(|| invalid("unknown owned lifecycle account"))?;
    if !stable_identifier(&public_ids.0)
        || !public_ids.0.starts_with("agt_")
        || !stable_identifier(&public_ids.1)
        || !public_ids.1.starts_with("aga_")
    {
        return Err(invalid("invalid registered lifecycle public identity"));
    }
    let mut request: Option<SpendRequest> = None;
    let mut policy_decision: Option<crate::policy::Effect> = None;
    let mut policy_identity: Option<(String, String)> = None;
    let admitted:Option<(String,String)> = conn.query_row("SELECT owner_user_id,request_json FROM spend_operation_attempts WHERE agent_id=?1 AND operation_key=?2 AND revision=?3",params![c.agent_id.to_string(),c.operation_key,c.attempt_revision],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
    if let Some((owner, json)) = admitted {
        if owner != c.owner_user_id.to_string() {
            return Err(invalid("lifecycle attempt scope mismatch"));
        }
        request = Some(serde_json::from_str(&json)?);
    }
    if let Some(id) = &c.decision_id {
        let stored:Option<(String,String,String,u64,String,String)>=conn.query_row("SELECT owner_user_id,agent_id,operation_key,revision,request_json,evaluation_json FROM spend_decisions WHERE id=?1",[id.to_string()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).optional()?;
        let (owner, agent, key, revision, json, evaluation) =
            stored.ok_or_else(|| invalid("unknown lifecycle decision"))?;
        if owner != c.owner_user_id.to_string()
            || agent != c.agent_id.to_string()
            || key != c.operation_key
            || revision != c.attempt_revision
        {
            return Err(invalid("lifecycle decision correlation mismatch"));
        }
        let decided: SpendRequest = serde_json::from_str(&json)?;
        if request
            .as_ref()
            .is_some_and(|r| !r.replay_equivalent(&decided))
        {
            return Err(invalid("lifecycle request mismatch"));
        }
        request = Some(decided);
        let evaluation: crate::policy::Evaluation = serde_json::from_str(&evaluation)?;
        policy_decision = Some(evaluation.decision);
        policy_identity = Some((evaluation.policy_id, evaluation.policy_version));
    }
    if let Some(r) = &request {
        if r.owner_user_id != c.owner_user_id
            || r.agent_id != c.agent_id
            || r.agent_account_id != c.agent_account_id
        {
            return Err(invalid("lifecycle request scope mismatch"));
        }
    }
    if request.is_none() {
        return Err(invalid("unknown lifecycle attempt"));
    }
    if let LifecycleEvidence::Known { value } = &input.requested_amount {
        if value.amount.is_negative()
            || request
                .as_ref()
                .is_some_and(|r| r.amount != value.amount || r.currency != value.currency)
        {
            return Err(invalid("lifecycle requested amount mismatch"));
        }
    }
    if let LifecycleEvidence::Known { value } = &input.execution_scope {
        let scope = request
            .as_ref()
            .and_then(|r| r.execution_scope.as_ref())
            .ok_or_else(|| invalid("lifecycle scope provenance unavailable"))?;
        if value.provider != scope.provider.id
            || value.executor != scope.executor.id
            || value.capability != scope.capability.id
            || value.billing_merchant != scope.billing_merchant.id
            || [
                &value.provider,
                &value.executor,
                &value.capability,
                &value.billing_merchant,
            ]
            .iter()
            .any(|s| !stable_identifier(s))
        {
            return Err(invalid("lifecycle scope mismatch"));
        }
    }
    match &input.fact {
        LifecycleFact::AutomatedAdmission { result } => {
            let compatible = match result {
                AutomatedAdmissionOutcome::Admitted => {
                    policy_decision == Some(crate::policy::Effect::Allow)
                }
                AutomatedAdmissionOutcome::Denied {
                    reason: AdmissionDenialReason::Policy,
                } => policy_decision == Some(crate::policy::Effect::Deny),
                AutomatedAdmissionOutcome::Denied {
                    reason: AdmissionDenialReason::Unknown,
                } => policy_decision.is_some(),
                AutomatedAdmissionOutcome::Denied { .. } => {
                    policy_decision == Some(crate::policy::Effect::Allow)
                }
            };
            if !compatible {
                return Err(invalid("lifecycle automated policy outcome mismatch"));
            }
            // Terminal admission facts follow their authoritative outcome in
            // the same caller transaction. Later claim/settlement/release does
            // not change that immutable authorization outcome.
            let expected = match result {
                AutomatedAdmissionOutcome::Admitted => "allowed",
                AutomatedAdmissionOutcome::Denied { .. } => "denied",
            };
            let canonical:Option<(String,Option<String>)>=conn.query_row("SELECT decision,spend_decision_id FROM spend_authorization_outcomes WHERE agent_id=?1 AND operation_key=?2 AND revision=?3 ORDER BY id DESC LIMIT 1",params![c.agent_id.to_string(),c.operation_key,c.attempt_revision],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
            if canonical.as_ref().is_none_or(|(decision, id)| {
                decision != expected
                    || id.as_ref() != c.decision_id.as_ref().map(ToString::to_string).as_ref()
            }) {
                return Err(invalid("lifecycle automated outcome is not confirmed"));
            }
        }
        LifecycleFact::InitialPolicy { decision } => {
            let expected = match decision {
                InitialPolicyDecision::Allow => crate::policy::Effect::Allow,
                InitialPolicyDecision::Deny => crate::policy::Effect::Deny,
                InitialPolicyDecision::NeedsApproval => crate::policy::Effect::NeedsApproval,
            };
            if policy_decision != Some(expected) {
                return Err(invalid("lifecycle initial policy mismatch"));
            }
        }
        LifecycleFact::HumanEscalation {
            approval_request_id,
        }
        | LifecycleFact::HumanResolutionConfirmed {
            approval_request_id,
            ..
        }
        | LifecycleFact::AuthorizationGranted {
            authorization_id: approval_request_id,
        }
        | LifecycleFact::AuthorizationReleased {
            authorization_id: approval_request_id,
            ..
        } => {
            if c.decision_id.as_ref() != Some(approval_request_id) {
                return Err(invalid("lifecycle authorization correlation mismatch"));
            }
            if matches!(
                input.fact,
                LifecycleFact::HumanEscalation { .. }
                    | LifecycleFact::HumanResolutionConfirmed { .. }
            ) && policy_decision != Some(crate::policy::Effect::NeedsApproval)
            {
                return Err(invalid("lifecycle human approval policy mismatch"));
            }
            if let LifecycleFact::HumanResolutionConfirmed {
                confirmed_by_user_id,
                ..
            } = &input.fact
            {
                if confirmed_by_user_id != &c.owner_user_id {
                    return Err(invalid("unconfirmed lifecycle human resolution"));
                }
            }
            if matches!(
                input.fact,
                LifecycleFact::AuthorizationGranted { .. }
                    | LifecycleFact::AuthorizationReleased { .. }
            ) {
                let token:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM spend_auth_tokens WHERE spend_decision_id=?1 AND owner_user_id=?2)",params![approval_request_id.to_string(),c.owner_user_id.to_string()],|row|row.get(0))?;
                if !token {
                    return Err(invalid("unknown lifecycle authorization"));
                }
                if policy_decision == Some(crate::policy::Effect::Deny) {
                    return Err(invalid("lifecycle authorization contradicts policy denial"));
                }
            }
            if let LifecycleFact::AuthorizationReleased { reason, .. } = &input.fact {
                let expected = if *reason == AuthorizationReleaseReason::Expired {
                    "expired"
                } else {
                    "released"
                };
                let released:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM budget_holds h JOIN spend_auth_tokens t ON t.spend_decision_id=h.spend_decision_id WHERE h.spend_decision_id=?1 AND h.status=?2 AND h.executor_claim_id IS NULL AND NOT EXISTS(SELECT 1 FROM spend_executor_claims cl WHERE cl.spend_auth_token_id=t.id))",params![approval_request_id.to_string(),expected],|row|row.get(0))?;
                if !released {
                    return Err(invalid("lifecycle authorization release is not confirmed"));
                }
            }
        }
        LifecycleFact::ExecutionStarted { claim_id }
        | LifecycleFact::ExecutionFailed { claim_id }
        | LifecycleFact::ExecutionFinalized { claim_id, .. }
        | LifecycleFact::ReconciliationRequired { claim_id }
        | LifecycleFact::ReconciliationResolved { claim_id, .. } => {
            let matched:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM spend_executor_claims cl JOIN spend_auth_tokens t ON t.id=cl.spend_auth_token_id WHERE cl.id=?1 AND cl.owner_user_id=?2 AND cl.agent_id=?3 AND cl.operation_key=?4 AND t.spend_decision_id=?5)",params![claim_id.to_string(),c.owner_user_id.to_string(),c.agent_id.to_string(),c.operation_key,c.decision_id.as_ref().map(ToString::to_string)],|row|row.get(0))?;
            if !matched {
                return Err(invalid("lifecycle claim correlation mismatch"));
            }
            let financial_outcome = match &input.fact {
                LifecycleFact::ExecutionFinalized { outcome, .. } => Some((*outcome, false)),
                LifecycleFact::ReconciliationResolved { outcome, .. } => Some((
                    match outcome {
                        ReconciliationResolution::VendorBilled => ExecutionFinalization::Settled,
                        ReconciliationResolution::VendorDidNotBill => {
                            ExecutionFinalization::Released
                        }
                    },
                    true,
                )),
                _ => None,
            };
            if let Some((outcome, reconciled)) = financial_outcome {
                let expected = match outcome {
                    ExecutionFinalization::Settled => "settled",
                    ExecutionFinalization::Released => "released",
                };
                let confirmed:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM spend_executor_claims cl JOIN budget_holds h ON h.executor_claim_id=cl.id WHERE cl.id=?1 AND cl.status=?2 AND h.status=?2 AND cl.finalized_at IS NOT NULL AND (?3=0 OR (cl.reconciled_at IS NOT NULL AND cl.reconciled_by_user_id=?4 AND length(trim(cl.reconciliation_evidence))>0)) AND (?2!='settled' OR EXISTS(SELECT 1 FROM spend_executor_settlement_receipts r WHERE r.claim_id=cl.id AND r.settlement_id=cl.settlement_id)))",params![claim_id.to_string(),expected,reconciled,c.owner_user_id.to_string()],|row|row.get(0))?;
                if !confirmed {
                    return Err(invalid("lifecycle financial outcome is not confirmed"));
                }
            }
        }
        _ => {}
    }
    if let LifecycleEvidence::Known { value } = &input.explanation.policy {
        if !stable_identifier(&value.policy_id)
            || !value.policy_id.starts_with("pol_")
            || value.revision == 0
            || !valid_hash(&value.payload_hash)
            || value
                .decisive_rule_ids
                .iter()
                .any(|id| !stable_identifier(id))
        {
            return Err(invalid("invalid lifecycle policy provenance"));
        }
        let policy_json:Option<String>=conn.query_row("SELECT r.policy_json FROM policy_revisions r JOIN policies p ON p.policy_id=r.policy_id WHERE p.owner_user_id=?1 AND r.policy_id=?2 AND r.revision=?3 AND r.payload_hash=?4",params![c.owner_user_id.to_string(),value.policy_id,value.revision,value.payload_hash],|row|row.get(0)).optional()?;
        let policy: Policy = serde_json::from_str(
            &policy_json.ok_or_else(|| invalid("unknown owned lifecycle policy revision"))?,
        )?;
        if policy_identity.as_ref() != Some(&(policy.id.clone(), policy.version.clone()))
            || value
                .decisive_rule_ids
                .iter()
                .any(|id| !policy.rules.iter().any(|rule| &rule.id == id))
        {
            return Err(invalid("lifecycle policy identity or rule mismatch"));
        }
        for constraint in &value.amount_constraints {
            if !stable_identifier(&constraint.rule_id)
                || !value.decisive_rule_ids.contains(&constraint.rule_id)
            {
                return Err(invalid("lifecycle constraint rule mismatch"));
            }
            if constraint.configured.amount.is_negative() {
                return Err(invalid("negative lifecycle policy threshold"));
            }
            if let LifecycleEvidence::Known { value } = &constraint.evaluated {
                if value.currency != constraint.configured.currency {
                    return Err(invalid("lifecycle constraint currency mismatch"));
                }
            }
        }
    }
    if let LifecycleEvidence::Known { value } = &input.explanation.budget {
        if !stable_identifier(&value.budget_id)
            || !value.budget_id.starts_with("bgt_")
            || !stable_identifier(&value.budget_version_id)
            || !value.budget_version_id.starts_with("bgv_")
            || value.requested.amount.is_negative()
        {
            return Err(invalid("invalid lifecycle budget provenance"));
        }
        let mut stmt=conn.prepare("SELECT b.id,v.id,v.amount_limit,b.currency FROM budgets b JOIN budget_versions v ON v.budget_id=b.id JOIN agent_accounts a ON a.agent_id=b.scope_id WHERE b.scope_type='agent' AND b.scope_id=?1 AND a.owner_user_id=?2 AND a.id=?3")?;
        let rows = stmt.query_map(
            params![
                c.agent_id.to_string(),
                c.owner_user_id.to_string(),
                c.agent_account_id.to_string()
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )?;
        let mut owned_version = None;
        for row in rows {
            let (budget, version, limit, currency) = row?;
            let budget: BudgetId = parse_id(&budget)?;
            let version: BudgetVersionId = parse_id(&version)?;
            if value.budget_id == format!("bgt_{}", budget.public_suffix())
                && value.budget_version_id == format!("bgv_{}", version.public_suffix())
            {
                owned_version = Some((Amount::from_units(limit), currency));
                break;
            }
        }
        let (limit, currency) =
            owned_version.ok_or_else(|| invalid("unknown owned lifecycle budget revision"))?;
        if currency != value.requested.currency.to_string() {
            return Err(invalid("lifecycle budget currency mismatch"));
        }
        if let LifecycleEvidence::Known {
            value: captured_limit,
        } = &value.limit
        {
            if captured_limit.amount != limit {
                return Err(invalid("lifecycle budget limit mismatch"));
            }
        }
        if request.as_ref().is_some_and(|r| {
            r.amount != value.requested.amount || r.currency != value.requested.currency
        }) {
            return Err(invalid("lifecycle budget request mismatch"));
        }
        if let LifecycleEvidence::Known { value: requested } = &input.requested_amount {
            if requested != &value.requested {
                return Err(invalid("lifecycle budget requested amount mismatch"));
            }
        }
        for amount in [
            &value.limit,
            &value.available,
            &value.consumed,
            &value.frozen,
        ] {
            if let LifecycleEvidence::Known { value: amount } = amount {
                if amount.currency != value.requested.currency {
                    return Err(invalid("lifecycle budget currency mismatch"));
                }
            }
        }
        for amount in [&value.consumed, &value.frozen] {
            if let LifecycleEvidence::Known { value: amount } = amount {
                if amount.amount.is_negative() {
                    return Err(invalid("negative lifecycle budget usage"));
                }
            }
        }
        if let (
            LifecycleEvidence::Known { value: limit },
            LifecycleEvidence::Known { value: available },
            LifecycleEvidence::Known { value: consumed },
            LifecycleEvidence::Known { value: frozen },
        ) = (
            &value.limit,
            &value.available,
            &value.consumed,
            &value.frozen,
        ) {
            if limit
                .amount
                .checked_sub(consumed.amount)
                .and_then(|v| v.checked_sub(frozen.amount))
                != Some(available.amount)
            {
                return Err(invalid("inconsistent lifecycle budget snapshot"));
            }
        }
    }
    Ok(public_ids)
}
fn stable_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_:-.".contains(&c))
}
fn valid_hash(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|h| {
        h.len() == 64
            && h.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
