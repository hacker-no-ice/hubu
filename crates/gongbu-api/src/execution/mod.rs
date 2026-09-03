//! SQLite persistence for the execution aggregate.
//! Schema units and formats are documented in the migration.
use crate::redaction::Redactor;
use crate::{
    execution_scope::{for_target, ExecutionScope},
    provider_contract::{
        ActualVendorCost, AsyncProviderOperation, NormalizedUsage, PollingRecoveryContext,
        PricingSnapshot, PRICING_SNAPSHOT_SCHEMA_VERSION,
    },
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::Value;
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use thiserror::Error;
use uuid::Uuid;

const MIGRATION: &str = include_str!("../../migrations/0001_execution_core.sql");
#[derive(Debug, Error)]
pub enum Error {
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("invalid {0}")]
    Invalid(&'static str),
    #[error("not found")]
    NotFound,
    #[error("ambiguous legacy Hubu token reference")]
    AmbiguousLegacyToken,
    #[error("stale version")]
    Stale,
    #[error("settlement exceeds authorization")]
    OverAuthorization,
    #[error("limit exceeded: {0}")]
    Limit(&'static str),
    #[error("forbidden execution transition from {from} to {to}")]
    ForbiddenTransition { from: String, to: String },
}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubuTokenReference(String);
impl HubuTokenReference {
    pub fn new(v: impl Into<String>) -> Result<Self> {
        let v = v.into();
        let v = v.trim();
        let l = v.to_ascii_lowercase();
        if v.is_empty()
            || v.len() > 255
            || l.starts_with("bearer ")
            || l.starts_with("eyj")
            || v.contains('.')
        {
            Err(Error::Invalid("raw Hubu token"))
        } else {
            Ok(Self(v.to_owned()))
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
#[derive(Clone, Debug)]
pub struct CreateExecutionParams {
    pub account_id: String,
    pub operation_key: String,
    pub hubu_authorization_id: String,
    pub hubu_claim_id: Option<String>,
    pub hubu_token_reference: HubuTokenReference,
    pub authorized_minor: i64,
    pub authorization_currency: String,
    pub normalized_input: Value,
    pub input_hash: String,
    pub input_schema_version: i64,
    pub target: String,
    pub config_version: String,
    pub workload_type: String,
    pub provider: String,
    pub adapter: String,
    pub model: String,
    pub provider_config_version: String,
    pub provider_config_digest: String,
    pub pricing_snapshot: Value,
    pub pricing_schema_version: i64,
    pub execution_scope: Option<ExecutionScope>,
    pub created_at: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubuAuthorizationSnapshot {
    pub account_id: String,
    pub agent_id: String,
    pub operation_key: String,
    pub decision_id: String,
    pub spend_auth_token_id: String,
    pub amount_minor: i64,
    pub currency: String,
    pub execution_scope: ExecutionScope,
    pub lease_profile: String,
    pub expires_at: String,
    pub authorization_status: String,
    pub task_id: Option<String>,
    pub reason: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Execution {
    pub execution_id: String,
    pub account_id: String,
    pub operation_key: String,
    pub hubu_authorization_id: String,
    pub hubu_claim_id: Option<String>,
    pub hubu_token_reference: HubuTokenReference,
    pub authorized_minor: i64,
    pub authorization_currency: String,
    pub normalized_input: Value,
    pub input_hash: String,
    pub input_schema_version: i64,
    pub target: String,
    pub config_version: String,
    pub workload_type: String,
    pub provider: String,
    pub adapter: String,
    pub model: String,
    pub provider_config_version: String,
    pub provider_config_digest: String,
    pub pricing_snapshot: Value,
    pub pricing_schema_version: i64,
    pub execution_scope: Option<ExecutionScope>,
    pub status: String,
    pub outcome: Option<String>,
    pub provider_outcome: Option<LifecycleOutcome>,
    pub artifact_outcome: Option<LifecycleOutcome>,
    pub settlement_outcome: Option<LifecycleOutcome>,
    pub failure_code: Option<String>,
    pub failure_message_redacted: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub release_transmission_started_at: Option<String>,
    pub version: i64,
}

#[derive(Clone, Copy, Debug)]
pub struct ExecutionSnapshot<'a> {
    pub execution_id: &'a str,
    pub account_id: &'a str,
    pub operation_key: &'a str,
    pub normalized_input: &'a Value,
    pub target: &'a str,
    pub pricing_snapshot: &'a Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleOutcome {
    Succeeded,
    Failed,
    Ambiguous,
    Released,
}

impl LifecycleOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Ambiguous => "ambiguous",
            Self::Released => "released",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "ambiguous" => Some(Self::Ambiguous),
            "released" => Some(Self::Released),
            _ => None,
        }
    }
}

impl Execution {
    pub fn snapshot(&self) -> ExecutionSnapshot<'_> {
        ExecutionSnapshot {
            execution_id: &self.execution_id,
            account_id: &self.account_id,
            operation_key: &self.operation_key,
            normalized_input: &self.normalized_input,
            target: &self.target,
            pricing_snapshot: &self.pricing_snapshot,
        }
    }
}
#[derive(Clone, Debug)]
pub struct ExecutionUpdate {
    pub status: String,
    pub outcome: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub failure_code: Option<String>,
    pub failure_message_redacted: Option<String>,
    pub provider_outcome: Option<LifecycleOutcome>,
    pub artifact_outcome: Option<LifecycleOutcome>,
    pub settlement_outcome: Option<LifecycleOutcome>,
}
#[derive(Clone, Debug)]
pub struct CreateProviderAttemptParams {
    pub execution_id: String,
    pub provider: String,
    pub provider_request_id: Option<String>,
    pub provider_operation_id: Option<String>,
    pub started_at: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderAttempt {
    pub provider_attempt_id: String,
    pub execution_id: String,
    pub provider: String,
    pub provider_request_id: Option<String>,
    pub provider_operation_id: Option<String>,
    pub provider_polling_host: Option<String>,
    pub provider_recovery_context: Option<PollingRecoveryContext>,
    pub provider_deadline_unix_ms: Option<i64>,
    pub operation_checkpointed_at: Option<String>,
    pub provider_poll_count: u64,
    pub artifact_fetch_count: u64,
    pub outcome: String,
    pub usage: Option<Value>,
    pub usage_schema_version: Option<i64>,
    pub actual_vendor_cost: Option<ActualVendorCost>,
    pub failure_code: Option<String>,
    pub failure_message_redacted: Option<String>,
    pub started_at: String,
    pub transmission_started_at: Option<String>,
    pub completed_at: Option<String>,
}
#[derive(Clone, Debug)]
pub struct AttemptResult {
    pub outcome: String,
    pub completed_at: String,
    pub usage: Value,
    pub usage_schema_version: i64,
    pub actual_vendor_cost: Option<ActualVendorCost>,
    pub failure_code: Option<String>,
    pub failure_message_redacted: Option<String>,
    pub provider_request_id: Option<String>,
    pub provider_operation_id: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedProviderArtifact {
    pub media_type: String,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct CreateArtifactParams {
    pub artifact_id: String,
    pub execution_id: String,
    pub provider_attempt_id: Option<String>,
    pub kind: String,
    pub storage_backend: String,
    pub media_type: String,
    pub storage_key: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub metadata: Value,
    pub metadata_schema_version: i64,
    pub created_at: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Artifact {
    pub artifact_id: String,
    pub execution_id: String,
    pub provider_attempt_id: Option<String>,
    pub kind: String,
    pub storage_backend: String,
    pub media_type: String,
    pub storage_key: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub metadata: Value,
    pub metadata_schema_version: i64,
    pub created_at: String,
}
#[derive(Clone, Debug)]
pub struct CreateReceiptParams {
    pub receipt_id: String,
    pub execution_id: String,
    pub provider_attempt_id: String,
    pub settlement_minor: i64,
    pub currency: String,
    pub pricing_catalog_version: String,
    pub actual_vendor_cost: ActualVendorCost,
    pub created_at: String,
    pub settled_at: Option<String>,
    pub hubu_settlement_id: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Receipt {
    pub receipt_id: String,
    pub execution_id: String,
    pub provider_attempt_id: String,
    pub settlement_minor: i64,
    pub currency: String,
    pub pricing_catalog_version: String,
    pub actual_vendor_cost: ActualVendorCost,
    /// Exact provider identity sent on the Hubu settlement wire. This may be
    /// the legacy deterministic receipt ID for receipts created before precise
    /// provider evidence was added.
    pub provider_request_id: String,
    /// Exact JSON value sent as `price_model_snapshot` on every Hubu retry.
    pub price_model_snapshot: Value,
    pub created_at: String,
    pub transmission_started_at: Option<String>,
    pub settled_at: Option<String>,
    pub hubu_settlement_id: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ReconciliationRecord {
    pub execution_id: String,
    pub evidence: Value,
    pub last_confirmed_step: String,
    pub entered_at: String,
    pub updated_at: String,
    pub automatic_attempts: i64,
    pub last_automatic_attempt_at: Option<String>,
    pub automatic_attempts_exhausted: bool,
    pub last_operator_action_id: Option<String>,
    pub last_operator_action: Option<String>,
}
#[derive(Clone)]
pub struct Repository(Arc<Mutex<Connection>>, Arc<Redactor>);
impl Repository {
    /// Persistent repositories require an explicitly configured redactor so the
    /// production path cannot silently omit operator credential registration.
    pub fn open(path: impl AsRef<Path>, redactor: Redactor) -> Result<Self> {
        Self::init(Connection::open(path)?, Arc::new(redactor))
    }
    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, Arc::new(Redactor::default()))
    }
    pub fn in_memory_with_redactor(redactor: Redactor) -> Result<Self> {
        Self::init(Connection::open_in_memory()?, Arc::new(redactor))
    }
    fn init(c: Connection, redactor: Arc<Redactor>) -> Result<Self> {
        c.pragma_update(None, "foreign_keys", "ON")?;
        c.pragma_update(None, "busy_timeout", 5000)?;
        c.execute_batch(MIGRATION)?;
        migrate_legacy_authorization_aliases(&c)?;
        migrate_artifact_storage_columns(&c)?;
        migrate_resolved_target_columns(&c)?;
        migrate_execution_scope_column(&c)?;
        migrate_precise_vendor_cost_columns(&c)?;
        migrate_provider_operation_checkpoint_columns(&c)?;
        migrate_provider_transport_counter_columns(&c)?;
        Ok(Self(Arc::new(Mutex::new(c)), redactor))
    }
    pub fn create_execution(&self, n: &CreateExecutionParams) -> Result<Execution> {
        self.create_execution_inner(n, None)
    }

    pub fn create_execution_with_authorization(
        &self,
        n: &CreateExecutionParams,
        authorization: &HubuAuthorizationSnapshot,
    ) -> Result<Execution> {
        self.create_execution_inner(n, Some(authorization))
    }

    fn create_execution_inner(
        &self,
        n: &CreateExecutionParams,
        authorization: Option<&HubuAuthorizationSnapshot>,
    ) -> Result<Execution> {
        validate_execution(n)?;
        if authorization.is_some_and(|authorization| {
            authorization.account_id != n.account_id
                || authorization.operation_key != n.operation_key
                || authorization.spend_auth_token_id != n.hubu_authorization_id
                || authorization.spend_auth_token_id != n.hubu_token_reference.as_str()
                || authorization.amount_minor != n.authorized_minor
                || !authorization
                    .currency
                    .eq_ignore_ascii_case(&n.authorization_currency)
                || n.execution_scope.as_ref() != Some(&authorization.execution_scope)
                || authorization.authorization_status != "available"
                || authorization.agent_id.trim().is_empty()
                || authorization.expires_at.trim().is_empty()
                || authorization.reason.trim().is_empty()
        }) {
            return Err(Error::Invalid("Hubu authorization snapshot"));
        }
        let id = Uuid::new_v4().to_string();
        self.reject_registered_secrets([
            n.account_id.as_str(),
            n.operation_key.as_str(),
            n.hubu_authorization_id.as_str(),
            n.hubu_claim_id.as_deref().unwrap_or(""),
            n.hubu_token_reference.0.as_str(),
            n.authorization_currency.as_str(),
            n.input_hash.as_str(),
            n.target.as_str(),
            n.config_version.as_str(),
            n.workload_type.as_str(),
            n.provider.as_str(),
            n.adapter.as_str(),
            n.model.as_str(),
            n.provider_config_version.as_str(),
            n.provider_config_digest.as_str(),
            n.created_at.as_str(),
            id.as_str(),
            "pending",
        ])?;
        self.reject_registered_numbers([
            n.authorized_minor,
            n.input_schema_version,
            n.pricing_schema_version,
            0,
        ])?;
        let normalized_input = j(&n.normalized_input);
        let pricing_snapshot = j(&n.pricing_snapshot);
        let execution_scope = n
            .execution_scope
            .as_ref()
            .map(|scope| serde_json::to_string(scope).expect("execution scope serializes"));
        self.reject_registered_json([&n.normalized_input, &n.pricing_snapshot])?;
        self.reject_registered_secrets([normalized_input.as_str(), pricing_snapshot.as_str()])?;
        let mut c = self.0.lock().unwrap();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if authorization.is_some() {
            match query_token(&tx, n.hubu_token_reference.as_str()) {
                Ok(existing) => return Ok(existing),
                Err(Error::NotFound) => {}
                Err(error) => return Err(error),
            }
        }
        let inserted = tx.execute("INSERT OR IGNORE INTO executions(execution_id,account_id,operation_key,hubu_authorization_id,hubu_claim_id,hubu_token_reference,authorized_minor,authorization_currency,normalized_input_json,input_hash,input_schema_version,target,config_version,workload_type,provider,adapter,model,provider_config_version,provider_config_digest,pricing_snapshot_json,pricing_schema_version,execution_scope_json,status,created_at,updated_at,version) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,'pending',?23,?23,0)",params![id,n.account_id,n.operation_key,n.hubu_authorization_id,n.hubu_claim_id,n.hubu_token_reference.0,n.authorized_minor,n.authorization_currency,j(&n.normalized_input),n.input_hash,n.input_schema_version,n.target,n.config_version,n.workload_type,n.provider,n.adapter,n.model,n.provider_config_version,n.provider_config_digest,j(&n.pricing_snapshot),n.pricing_schema_version,execution_scope,n.created_at])?;
        let e = if inserted == 1 {
            query_id(&tx, &id)?
        } else {
            query_key(&tx, &n.account_id, &n.operation_key)?
        };
        if let Some(authorization) = authorization {
            if e.execution_id == id {
                tx.execute(
                    "INSERT INTO hubu_authorization_snapshots(execution_id,account_id,agent_id,operation_key,decision_id,spend_auth_token_id,amount_minor,currency,execution_scope_json,lease_profile,expires_at,authorization_status,task_id,reason) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
                    params![e.execution_id,authorization.account_id,authorization.agent_id,authorization.operation_key,authorization.decision_id,authorization.spend_auth_token_id,authorization.amount_minor,authorization.currency,serde_json::to_string(&authorization.execution_scope).expect("execution scope serializes"),authorization.lease_profile,authorization.expires_at,authorization.authorization_status,authorization.task_id,authorization.reason],
                )?;
            }
        }
        tx.commit()?;
        Ok(e)
    }
    pub fn get_execution(&self, id: &str) -> Result<Execution> {
        query_id(&self.0.lock().unwrap(), id)
    }
    pub fn get_execution_by_operation(
        &self,
        account_id: &str,
        operation_key: &str,
    ) -> Result<Execution> {
        self.0
            .lock()
            .unwrap()
            .query_row(
                &format!("{EXECUTION_SELECT} WHERE account_id=?1 AND operation_key=?2"),
                params![account_id, operation_key],
                map,
            )
            .optional()?
            .ok_or(Error::NotFound)
    }

    pub fn get_execution_by_hubu_token(
        &self,
        account_id: &str,
        hubu_token_reference: &str,
    ) -> Result<Execution> {
        self.0
            .lock()
            .unwrap()
            .query_row(
                &format!("{EXECUTION_SELECT} WHERE account_id=?1 AND hubu_token_reference=?2"),
                params![account_id, hubu_token_reference],
                map,
            )
            .optional()?
            .ok_or(Error::NotFound)
    }

    pub fn get_execution_by_spend_auth_token(
        &self,
        spend_auth_token_id: &str,
    ) -> Result<Execution> {
        query_token(&self.0.lock().unwrap(), spend_auth_token_id)
    }

    #[cfg(test)]
    pub(crate) fn delete_hubu_authorization_snapshot(
        &self,
        execution_id: &str,
    ) -> rusqlite::Result<usize> {
        self.0.lock().unwrap().execute(
            "DELETE FROM hubu_authorization_snapshots WHERE execution_id=?1",
            [execution_id],
        )
    }

    pub fn get_hubu_authorization_snapshot(
        &self,
        execution_id: &str,
    ) -> Result<HubuAuthorizationSnapshot> {
        self.0
            .lock()
            .unwrap()
            .query_row(
                "SELECT account_id,agent_id,operation_key,decision_id,spend_auth_token_id,amount_minor,currency,execution_scope_json,lease_profile,expires_at,authorization_status,task_id,reason FROM hubu_authorization_snapshots WHERE execution_id=?1",
                [execution_id],
                |row| {
                    let scope: String = row.get(7)?;
                    Ok(HubuAuthorizationSnapshot {
                        account_id: row.get(0)?,
                        agent_id: row.get(1)?,
                        operation_key: row.get(2)?,
                        decision_id: row.get(3)?,
                        spend_auth_token_id: row.get(4)?,
                        amount_minor: row.get(5)?,
                        currency: row.get(6)?,
                        execution_scope: serde_json::from_str(&scope).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                scope.len(),
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?,
                        lease_profile: row.get(8)?,
                        expires_at: row.get(9)?,
                        authorization_status: row.get(10)?,
                        task_id: row.get(11)?,
                        reason: row.get(12)?,
                    })
                },
            )
            .optional()?
            .ok_or(Error::NotFound)
    }

    /// Stable execution IDs that must have a live or restartable Temporal
    /// workflow after process restart. Rescheduling uses Temporal's UseExisting
    /// conflict policy and therefore cannot create a second live run.
    pub fn list_nonterminal_execution_ids(&self) -> Result<Vec<String>> {
        let connection = self.0.lock().unwrap();
        let mut statement = connection.prepare(
            "SELECT execution_id FROM executions WHERE status NOT IN ('succeeded','failed','released') ORDER BY created_at, execution_id",
        )?;
        let rows = statement.query_map([], |row| row.get(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
    pub fn update_execution(
        &self,
        id: &str,
        expected: i64,
        u: &ExecutionUpdate,
        at: &str,
    ) -> Result<Execution> {
        status(&u.status)?;
        let c = self.0.lock().unwrap();
        let failure = u
            .failure_message_redacted
            .as_deref()
            .map(|value| self.1.redact(value));
        self.reject_registered_secrets([
            u.status.as_str(),
            u.outcome.as_deref().unwrap_or(""),
            u.started_at.as_deref().unwrap_or(""),
            u.completed_at.as_deref().unwrap_or(""),
            u.failure_code.as_deref().unwrap_or(""),
            failure.as_deref().unwrap_or(""),
            at,
            id,
        ])?;
        self.reject_registered_numbers([expected.saturating_add(1)])?;
        let current: Option<String> = c
            .query_row(
                "SELECT status FROM executions WHERE execution_id=?1 AND version=?2",
                params![id, expected],
                |r| r.get(0),
            )
            .optional()?;
        let Some(current) = current else {
            return if c
                .query_row(
                    "SELECT 1 FROM executions WHERE execution_id=?1",
                    [id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some()
            {
                Err(Error::Stale)
            } else {
                Err(Error::NotFound)
            };
        };
        if !allowed_transition(&current, &u.status) {
            return Err(Error::ForbiddenTransition {
                from: current,
                to: u.status.clone(),
            });
        }
        let provider_outcome = u.provider_outcome.map(LifecycleOutcome::as_str);
        let artifact_outcome = u.artifact_outcome.map(LifecycleOutcome::as_str);
        let settlement_outcome = u.settlement_outcome.map(LifecycleOutcome::as_str);
        let changed=c.execute("UPDATE executions SET status=?1,outcome=?2,started_at=COALESCE(started_at,?3),completed_at=?4,failure_code=?5,failure_message_redacted=?6,updated_at=?7,provider_outcome=COALESCE(?8,provider_outcome),artifact_outcome=COALESCE(?9,artifact_outcome),settlement_outcome=COALESCE(?10,settlement_outcome),version=version+1 WHERE execution_id=?11 AND version=?12",params![u.status,u.outcome,u.started_at,u.completed_at,u.failure_code,failure,at,provider_outcome,artifact_outcome,settlement_outcome,id,expected])?;
        if changed == 0 {
            return if c
                .query_row(
                    "SELECT 1 FROM executions WHERE execution_id=?1",
                    [id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some()
            {
                Err(Error::Stale)
            } else {
                Err(Error::NotFound)
            };
        }
        query_id(&c, id)
    }
    pub fn set_claim(
        &self,
        id: &str,
        expected: i64,
        claim_id: &str,
        at: &str,
    ) -> Result<Execution> {
        let c = self.0.lock().unwrap();
        let changed = c.execute("UPDATE executions SET hubu_claim_id=?1,status='claimed',updated_at=?2,version=version+1 WHERE execution_id=?3 AND version=?4 AND status='preflighting' AND hubu_claim_id IS NULL",params![claim_id,at,id,expected])?;
        if changed == 0 {
            return Err(Error::Stale);
        }
        query_id(&c, id)
    }
    pub fn accept_existing_claim(&self, id: &str, expected: i64, at: &str) -> Result<Execution> {
        let c = self.0.lock().unwrap();
        let changed = c.execute("UPDATE executions SET status='claimed',updated_at=?1,version=version+1 WHERE execution_id=?2 AND version=?3 AND status='preflighting' AND hubu_claim_id IS NOT NULL", params![at,id,expected])?;
        if changed == 0 {
            return Err(Error::Stale);
        }
        query_id(&c, id)
    }
    pub fn set_reconciliation_claim(
        &self,
        id: &str,
        expected: i64,
        claim_id: &str,
        at: &str,
    ) -> Result<Execution> {
        let c = self.0.lock().unwrap();
        let changed=c.execute("UPDATE executions SET hubu_claim_id=?1,updated_at=?2,version=version+1 WHERE execution_id=?3 AND version=?4 AND status='reconciliation_required' AND hubu_claim_id IS NULL",params![claim_id,at,id,expected])?;
        if changed != 1 {
            return Err(Error::Stale);
        }
        query_id(&c, id)
    }
    pub fn begin_release_transmission(
        &self,
        id: &str,
        expected: i64,
        at: &str,
    ) -> Result<Execution> {
        let c = self.0.lock().unwrap();
        let changed = c.execute("UPDATE executions SET release_transmission_started_at=?1,updated_at=?1,version=version+1 WHERE execution_id=?2 AND version=?3 AND status='executing' AND release_transmission_started_at IS NULL", params![at,id,expected])?;
        if changed == 0 {
            return Err(Error::Stale);
        }
        query_id(&c, id)
    }
    pub fn create_provider_attempt(
        &self,
        n: &CreateProviderAttemptParams,
    ) -> Result<ProviderAttempt> {
        if n.provider.trim().is_empty() {
            return Err(Error::Invalid("provider"));
        }
        let id = Uuid::new_v4().to_string();
        self.reject_registered_secrets([
            n.execution_id.as_str(),
            n.provider.as_str(),
            n.provider_request_id.as_deref().unwrap_or(""),
            n.provider_operation_id.as_deref().unwrap_or(""),
            n.started_at.as_str(),
            id.as_str(),
            "started",
        ])?;
        self.0.lock().unwrap().execute("INSERT INTO provider_attempts(provider_attempt_id,execution_id,provider,provider_request_id,provider_operation_id,outcome,started_at,transmission_started_at)VALUES(?1,?2,?3,?4,?5,'started',?6,?6)",params![id,n.execution_id,n.provider,n.provider_request_id,n.provider_operation_id,n.started_at])?;
        Ok(ProviderAttempt {
            provider_attempt_id: id,
            execution_id: n.execution_id.clone(),
            provider: n.provider.clone(),
            provider_request_id: n.provider_request_id.clone(),
            provider_operation_id: n.provider_operation_id.clone(),
            provider_polling_host: None,
            provider_recovery_context: None,
            provider_deadline_unix_ms: None,
            operation_checkpointed_at: None,
            provider_poll_count: 0,
            artifact_fetch_count: 0,
            outcome: "started".into(),
            usage: None,
            usage_schema_version: None,
            actual_vendor_cost: None,
            failure_code: None,
            failure_message_redacted: None,
            started_at: n.started_at.clone(),
            transmission_started_at: Some(n.started_at.clone()),
            completed_at: None,
        })
    }
    pub fn start_provider_attempt(
        &self,
        execution: &Execution,
        at: &str,
    ) -> Result<ProviderAttempt> {
        let mut c = self.0.lock().unwrap();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: String = tx
            .query_row(
                "SELECT status FROM executions WHERE execution_id=?1 AND version=?2",
                params![execution.execution_id, execution.version],
                |r| r.get(0),
            )
            .optional()?
            .ok_or(Error::Stale)?;
        if current != "claimed" {
            return Err(Error::ForbiddenTransition {
                from: current,
                to: "executing".into(),
            });
        }
        let existing: i64 = tx.query_row(
            "SELECT count(*) FROM provider_attempts WHERE execution_id=?1",
            [&execution.execution_id],
            |r| r.get(0),
        )?;
        if existing != 0 {
            return Err(Error::Invalid("execution already has provider attempt"));
        }
        let id = Uuid::new_v4().to_string();
        tx.execute("INSERT INTO provider_attempts(provider_attempt_id,execution_id,provider,outcome,started_at) VALUES(?1,?2,?3,'started',?4)",params![id,execution.execution_id,execution.provider,at])?;
        tx.execute("UPDATE executions SET status='executing',started_at=COALESCE(started_at,?1),updated_at=?1,version=version+1 WHERE execution_id=?2 AND version=?3",params![at,execution.execution_id,execution.version])?;
        tx.commit()?;
        drop(c);
        self.get_provider_attempt_for_execution(&execution.execution_id)
    }
    pub fn begin_provider_transmission(&self, attempt_id: &str, at: &str) -> Result<()> {
        let changed=self.0.lock().unwrap().execute("UPDATE provider_attempts SET transmission_started_at=?1 WHERE provider_attempt_id=?2 AND transmission_started_at IS NULL AND completed_at IS NULL",params![at,attempt_id])?;
        if changed == 1 {
            Ok(())
        } else {
            Err(Error::Stale)
        }
    }
    /// Durably record entry into one provider poll transport call. The update
    /// precedes the transport invocation, so a worker crash can never make an
    /// attempted provider interaction disappear from recovery evidence.
    pub fn record_provider_poll(&self, attempt_id: &str) -> Result<()> {
        self.increment_provider_transport_counter(attempt_id, "provider_poll_count")
    }

    /// Durably record entry into one provider artifact-fetch transport call.
    pub fn record_artifact_fetch(&self, attempt_id: &str) -> Result<()> {
        self.increment_provider_transport_counter(attempt_id, "artifact_fetch_count")
    }

    fn increment_provider_transport_counter(
        &self,
        attempt_id: &str,
        column: &'static str,
    ) -> Result<()> {
        let changed = self.0.lock().unwrap().execute(
            &format!(
                "UPDATE provider_attempts SET {column}={column}+1 WHERE provider_attempt_id=?1 AND outcome='started' AND completed_at IS NULL AND transmission_started_at IS NOT NULL AND {column}<9223372036854775807"
            ),
            [attempt_id],
        )?;
        if changed == 1 {
            Ok(())
        } else {
            Err(Error::Stale)
        }
    }
    /// Atomically checkpoint the safe state needed to resume an accepted
    /// asynchronous provider operation. Identical redelivery is idempotent;
    /// conflicting evidence is rejected and can never cause a second submit.
    pub fn record_provider_operation(
        &self,
        attempt_id: &str,
        operation: &AsyncProviderOperation,
        checkpointed_at: &str,
    ) -> Result<ProviderAttempt> {
        operation
            .validate()
            .map_err(|_| Error::Invalid("provider operation checkpoint"))?;
        self.reject_registered_secrets([
            attempt_id,
            operation.provider_request_id.as_deref().unwrap_or(""),
            operation.provider_operation_id.as_str(),
            operation.polling_host.as_str(),
            operation
                .polling_recovery
                .as_ref()
                .and_then(|context| context.normalized_host.as_deref())
                .unwrap_or(""),
            checkpointed_at,
        ])?;
        if let Some(context) = operation.polling_recovery.as_ref() {
            let context = serde_json::to_value(context)
                .map_err(|_| Error::Invalid("provider operation checkpoint"))?;
            self.reject_registered_json([&context])?;
        }
        self.reject_registered_numbers([operation.deadline_unix_ms])?;

        let connection = self.0.lock().unwrap();
        let changed = connection.execute(
            "UPDATE provider_attempts SET provider_request_id=?1,provider_operation_id=?2,provider_polling_host=?3,provider_deadline_unix_ms=?4,operation_checkpointed_at=?5,provider_recovery_context_json=?6 WHERE provider_attempt_id=?7 AND outcome='started' AND completed_at IS NULL AND transmission_started_at IS NOT NULL AND provider_operation_id IS NULL AND provider_polling_host IS NULL AND provider_deadline_unix_ms IS NULL AND operation_checkpointed_at IS NULL",
            params![
                operation.provider_request_id,
                operation.provider_operation_id,
                operation.polling_host,
                operation.deadline_unix_ms,
                checkpointed_at,
                operation
                    .polling_recovery
                    .as_ref()
                    .map(|context| serde_json::to_string(context).expect("validated recovery context serializes")),
                attempt_id
            ],
        )?;
        drop(connection);
        if changed == 1 {
            return self.get_provider_attempt(attempt_id);
        }

        let existing = self.get_provider_attempt(attempt_id)?;
        if existing.outcome == "started"
            && existing.completed_at.is_none()
            && existing.transmission_started_at.is_some()
            && existing.provider_request_id == operation.provider_request_id
            && existing.provider_operation_id.as_deref()
                == Some(operation.provider_operation_id.as_str())
            && existing.provider_polling_host.as_deref() == Some(operation.polling_host.as_str())
            && existing.provider_recovery_context == operation.polling_recovery
            && existing.provider_deadline_unix_ms == Some(operation.deadline_unix_ms)
            && existing.operation_checkpointed_at.as_deref() == Some(checkpointed_at)
        {
            Ok(existing)
        } else {
            Err(Error::Stale)
        }
    }

    pub fn provider_operation(
        &self,
        attempt: &ProviderAttempt,
    ) -> Result<Option<AsyncProviderOperation>> {
        match (
            attempt.provider_operation_id.as_ref(),
            attempt.provider_polling_host.as_ref(),
            attempt.provider_deadline_unix_ms,
            attempt.operation_checkpointed_at.as_ref(),
        ) {
            (None, None, None, None) => Ok(None),
            (Some(operation_id), Some(polling_host), Some(deadline_unix_ms), Some(_)) => {
                let operation = AsyncProviderOperation {
                    provider_request_id: attempt.provider_request_id.clone(),
                    provider_operation_id: operation_id.clone(),
                    polling_host: polling_host.clone(),
                    polling_recovery: attempt.provider_recovery_context.clone(),
                    deadline_unix_ms,
                };
                operation
                    .validate()
                    .map_err(|_| Error::Invalid("provider operation checkpoint"))?;
                Ok(Some(operation))
            }
            _ => Err(Error::Invalid("provider operation checkpoint")),
        }
    }

    /// Atomically reopen only a checkpointed provider attempt whose original
    /// origin rejection is durably recoverable. Later ambiguous GET failures
    /// remain eligible because the immutable recovery reason, rather than the
    /// most recent failure code, is the authority for this path. No submission
    /// state is cleared and no new provider attempt can be created.
    pub fn begin_provider_reconciliation_poll(
        &self,
        execution_id: &str,
        attempt_id: &str,
        expected_execution_version: i64,
        at: &str,
    ) -> Result<Execution> {
        let mut connection = self.0.lock().unwrap();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let attempt_changed = transaction.execute(
            "UPDATE provider_attempts SET outcome='started',completed_at=NULL,failure_code=NULL,failure_message_redacted=NULL WHERE provider_attempt_id=?1 AND execution_id=?2 AND outcome='ambiguous' AND completed_at IS NOT NULL AND provider_operation_id IS NOT NULL AND provider_polling_host IS NOT NULL AND provider_deadline_unix_ms IS NOT NULL AND operation_checkpointed_at IS NOT NULL AND json_extract(provider_recovery_context_json,'$.validation_reason')='host_not_allowlisted'",
            params![attempt_id, execution_id],
        )?;
        if attempt_changed != 1 {
            return Err(Error::Stale);
        }
        let execution_changed = transaction.execute(
            "UPDATE executions SET status='executing',outcome=NULL,failure_code=NULL,failure_message_redacted=NULL,completed_at=NULL,updated_at=?1,version=version+1 WHERE execution_id=?2 AND version=?3 AND status='reconciliation_required'",
            params![at, execution_id, expected_execution_version],
        )?;
        if execution_changed != 1 {
            return Err(Error::Stale);
        }
        transaction.commit()?;
        drop(connection);
        self.get_execution(execution_id)
    }
    pub fn complete_provider_attempt(&self, id: &str, r: &AttemptResult) -> Result<()> {
        safe_usage_json(&r.usage)?;
        if r.usage_schema_version < 1
            || r.actual_vendor_cost
                .as_ref()
                .is_some_and(|cost| cost.validate().is_err())
        {
            return Err(Error::Invalid("attempt result"));
        }
        let failure = r
            .failure_message_redacted
            .as_deref()
            .map(|value| self.1.redact(value));
        let usage = j(&r.usage);
        let numeric = [
            Some(r.usage_schema_version),
            r.actual_vendor_cost.as_ref().map(|cost| cost.amount),
            r.actual_vendor_cost
                .as_ref()
                .map(|cost| i64::from(cost.scale)),
        ];
        self.reject_registered_numbers(numeric.into_iter().flatten())?;
        self.reject_registered_json([&r.usage])?;
        self.reject_registered_secrets([
            r.outcome.as_str(),
            r.completed_at.as_str(),
            usage.as_str(),
            r.actual_vendor_cost
                .as_ref()
                .map(|cost| cost.currency.as_str())
                .unwrap_or(""),
            r.failure_code.as_deref().unwrap_or(""),
            failure.as_deref().unwrap_or(""),
            r.provider_request_id.as_deref().unwrap_or(""),
            r.provider_operation_id.as_deref().unwrap_or(""),
            id,
        ])?;
        let compatibility_minor = r
            .actual_vendor_cost
            .as_ref()
            .map(|cost| cost.to_budget_minor_units(&cost.currency))
            .transpose()
            .map_err(|_| Error::Invalid("attempt result"))?;
        let n=self.0.lock().unwrap().execute("UPDATE provider_attempts SET outcome=?1,completed_at=?2,usage_json=?3,usage_schema_version=?4,provider_amount_minor=?5,provider_currency=?6,actual_vendor_cost_amount=?7,actual_vendor_cost_scale=?8,actual_vendor_cost_currency=?9,failure_code=?10,failure_message_redacted=?11,provider_request_id=?12,provider_operation_id=?13 WHERE provider_attempt_id=?14 AND completed_at IS NULL AND transmission_started_at IS NOT NULL",params![r.outcome,r.completed_at,j(&r.usage),r.usage_schema_version,compatibility_minor,r.actual_vendor_cost.as_ref().map(|cost| &cost.currency),r.actual_vendor_cost.as_ref().map(|cost| cost.amount),r.actual_vendor_cost.as_ref().map(|cost| cost.scale),r.actual_vendor_cost.as_ref().map(|cost| &cost.currency),r.failure_code,failure,r.provider_request_id,r.provider_operation_id,id])?;
        if n == 1 {
            Ok(())
        } else {
            Err(Error::NotFound)
        }
    }
    pub fn complete_provider_attempt_with_artifacts(
        &self,
        id: &str,
        r: &AttemptResult,
        artifacts: &[StagedProviderArtifact],
    ) -> Result<()> {
        if artifacts.is_empty() {
            return Err(Error::Invalid("staged provider artifacts"));
        }
        safe_usage_json(&r.usage)?;
        if r.outcome != "succeeded"
            || r.usage_schema_version < 1
            || r.actual_vendor_cost
                .as_ref()
                .is_some_and(|cost| cost.validate().is_err())
            || artifacts
                .iter()
                .any(|artifact| artifact.media_type.trim().is_empty())
        {
            return Err(Error::Invalid("attempt result"));
        }
        let usage = j(&r.usage);
        self.reject_registered_numbers(
            [
                Some(r.usage_schema_version),
                r.actual_vendor_cost.as_ref().map(|cost| cost.amount),
                r.actual_vendor_cost
                    .as_ref()
                    .map(|cost| i64::from(cost.scale)),
            ]
            .into_iter()
            .flatten(),
        )?;
        self.reject_registered_json([&r.usage])?;
        self.reject_registered_secrets([
            r.outcome.as_str(),
            r.completed_at.as_str(),
            usage.as_str(),
            r.actual_vendor_cost
                .as_ref()
                .map(|cost| cost.currency.as_str())
                .unwrap_or(""),
            r.provider_request_id.as_deref().unwrap_or(""),
            r.provider_operation_id.as_deref().unwrap_or(""),
            id,
        ])?;
        let mut connection = self.0.lock().unwrap();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let compatibility_minor = r
            .actual_vendor_cost
            .as_ref()
            .map(|cost| cost.to_budget_minor_units(&cost.currency))
            .transpose()
            .map_err(|_| Error::Invalid("attempt result"))?;
        let changed = transaction.execute(
            "UPDATE provider_attempts SET outcome=?1,completed_at=?2,usage_json=?3,usage_schema_version=?4,provider_amount_minor=?5,provider_currency=?6,actual_vendor_cost_amount=?7,actual_vendor_cost_scale=?8,actual_vendor_cost_currency=?9,failure_code=NULL,failure_message_redacted=NULL,provider_request_id=?10,provider_operation_id=?11 WHERE provider_attempt_id=?12 AND completed_at IS NULL AND transmission_started_at IS NOT NULL",
            params![r.outcome,r.completed_at,usage,r.usage_schema_version,compatibility_minor,r.actual_vendor_cost.as_ref().map(|cost| &cost.currency),r.actual_vendor_cost.as_ref().map(|cost| cost.amount),r.actual_vendor_cost.as_ref().map(|cost| cost.scale),r.actual_vendor_cost.as_ref().map(|cost| &cost.currency),r.provider_request_id,r.provider_operation_id,id],
        )?;
        if changed != 1 {
            return Err(Error::NotFound);
        }
        for (ordinal, artifact) in artifacts.iter().enumerate() {
            transaction.execute(
                "INSERT INTO staged_provider_artifacts(provider_attempt_id,ordinal,media_type,bytes) VALUES(?1,?2,?3,?4)",
                params![id, i64::try_from(ordinal).map_err(|_| Error::Limit("staged artifact count"))?, artifact.media_type, artifact.bytes],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }
    pub fn get_staged_provider_artifacts(
        &self,
        provider_attempt_id: &str,
    ) -> Result<Vec<StagedProviderArtifact>> {
        let connection = self.0.lock().unwrap();
        let mut statement = connection.prepare(
            "SELECT media_type,bytes FROM staged_provider_artifacts WHERE provider_attempt_id=?1 ORDER BY ordinal",
        )?;
        let artifacts = statement
            .query_map([provider_attempt_id], |row| {
                Ok(StagedProviderArtifact {
                    media_type: row.get(0)?,
                    bytes: row.get(1)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(artifacts)
    }
    pub fn complete_artifact_persistence(
        &self,
        execution: &Execution,
        provider_attempt_id: &str,
        at: &str,
    ) -> Result<Execution> {
        self.reject_registered_secrets([
            execution.execution_id.as_str(),
            provider_attempt_id,
            at,
            "settling",
            "succeeded",
        ])?;
        self.reject_registered_numbers([execution.version.saturating_add(1)])?;
        let mut connection = self.0.lock().unwrap();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE executions SET status='settling',outcome=NULL,started_at=COALESCE(started_at,?1),completed_at=NULL,failure_code=NULL,failure_message_redacted=NULL,updated_at=?1,provider_outcome='succeeded',artifact_outcome='succeeded',version=version+1 WHERE execution_id=?2 AND version=?3 AND status='executing' AND EXISTS(SELECT 1 FROM provider_attempts WHERE provider_attempt_id=?4 AND execution_id=?2 AND outcome='succeeded' AND completed_at IS NOT NULL)",
            params![at, execution.execution_id, execution.version, provider_attempt_id],
        )?;
        if changed != 1 {
            return Err(Error::Stale);
        }
        transaction.execute(
            "DELETE FROM staged_provider_artifacts WHERE provider_attempt_id=?1",
            [provider_attempt_id],
        )?;
        transaction.commit()?;
        query_id(&connection, &execution.execution_id)
    }
    pub fn get_provider_attempt_for_execution(
        &self,
        execution_id: &str,
    ) -> Result<ProviderAttempt> {
        self.0.lock().unwrap().query_row("SELECT provider_attempt_id,execution_id,provider,provider_request_id,provider_operation_id,provider_polling_host,provider_deadline_unix_ms,operation_checkpointed_at,provider_poll_count,artifact_fetch_count,outcome,usage_json,usage_schema_version,actual_vendor_cost_amount,actual_vendor_cost_scale,actual_vendor_cost_currency,failure_code,failure_message_redacted,started_at,transmission_started_at,completed_at,provider_recovery_context_json FROM provider_attempts WHERE execution_id=?1", [execution_id], |r| {
            let usage: Option<String> = r.get(11)?;
            let recovery:Option<String>=r.get(21)?;
            Ok(ProviderAttempt { provider_attempt_id:r.get(0)?, execution_id:r.get(1)?, provider:r.get(2)?, provider_request_id:r.get(3)?, provider_operation_id:r.get(4)?, provider_polling_host:r.get(5)?, provider_recovery_context:recovery.map(|value| serde_json::from_str(&value)).transpose().map_err(|error| rusqlite::Error::FromSqlConversionFailure(21,rusqlite::types::Type::Text,Box::new(error)))?, provider_deadline_unix_ms:r.get(6)?, operation_checkpointed_at:r.get(7)?, provider_poll_count:r.get(8)?, artifact_fetch_count:r.get(9)?, outcome:r.get(10)?, usage:usage.map(|v| serde_json::from_str(&v).unwrap()), usage_schema_version:r.get(12)?, actual_vendor_cost:actual_vendor_cost_from_row(r,13,14,15)?, failure_code:r.get(16)?, failure_message_redacted:r.get(17)?, started_at:r.get(18)?, transmission_started_at:r.get(19)?, completed_at:r.get(20)? })
        }).optional()?.ok_or(Error::NotFound)
    }
    pub fn count_provider_attempts_for_execution(&self, execution_id: &str) -> Result<u64> {
        self.0
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM provider_attempts WHERE execution_id=?1",
                [execution_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }
    pub fn get_provider_attempt(&self, provider_attempt_id: &str) -> Result<ProviderAttempt> {
        self.0.lock().unwrap().query_row("SELECT provider_attempt_id,execution_id,provider,provider_request_id,provider_operation_id,provider_polling_host,provider_deadline_unix_ms,operation_checkpointed_at,provider_poll_count,artifact_fetch_count,outcome,usage_json,usage_schema_version,actual_vendor_cost_amount,actual_vendor_cost_scale,actual_vendor_cost_currency,failure_code,failure_message_redacted,started_at,transmission_started_at,completed_at,provider_recovery_context_json FROM provider_attempts WHERE provider_attempt_id=?1", [provider_attempt_id], |r| {
            let usage: Option<String> = r.get(11)?;
            let recovery:Option<String>=r.get(21)?;
            Ok(ProviderAttempt { provider_attempt_id:r.get(0)?, execution_id:r.get(1)?, provider:r.get(2)?, provider_request_id:r.get(3)?, provider_operation_id:r.get(4)?, provider_polling_host:r.get(5)?, provider_recovery_context:recovery.map(|value| serde_json::from_str(&value)).transpose().map_err(|error| rusqlite::Error::FromSqlConversionFailure(21,rusqlite::types::Type::Text,Box::new(error)))?, provider_deadline_unix_ms:r.get(6)?, operation_checkpointed_at:r.get(7)?, provider_poll_count:r.get(8)?, artifact_fetch_count:r.get(9)?, outcome:r.get(10)?, usage:usage.map(|v| serde_json::from_str(&v).unwrap()), usage_schema_version:r.get(12)?, actual_vendor_cost:actual_vendor_cost_from_row(r,13,14,15)?, failure_code:r.get(16)?, failure_message_redacted:r.get(17)?, started_at:r.get(18)?, transmission_started_at:r.get(19)?, completed_at:r.get(20)? })
        }).optional()?.ok_or(Error::NotFound)
    }
    pub fn create_artifact(&self, n: &CreateArtifactParams) -> Result<Artifact> {
        self.create_artifact_with_limit(n, u64::MAX)
    }
    pub fn create_artifact_with_limit(
        &self,
        n: &CreateArtifactParams,
        max_per_execution: u64,
    ) -> Result<Artifact> {
        let metadata = j(&n.metadata);
        self.reject_registered_numbers([n.size_bytes, n.metadata_schema_version])?;
        self.reject_registered_json([&n.metadata])?;
        self.reject_registered_secrets([
            n.artifact_id.as_str(),
            n.execution_id.as_str(),
            n.provider_attempt_id.as_deref().unwrap_or(""),
            n.kind.as_str(),
            n.storage_backend.as_str(),
            n.media_type.as_str(),
            n.storage_key.as_str(),
            n.sha256.as_str(),
            metadata.as_str(),
            n.created_at.as_str(),
        ])?;
        safe_json(&n.metadata)?;
        if n.artifact_id.is_empty()
            || !n
                .artifact_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || n.storage_backend != "local_fs"
            || n.size_bytes < 0
            || n.metadata_schema_version < 1
        {
            return Err(Error::Invalid("artifact"));
        }
        let extension = match n.media_type.as_str() {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            _ => return Err(Error::Invalid("artifact media type")),
        };
        if n.kind != "image"
            || n.storage_key
                != format!(
                    "executions/{}/{}.{}",
                    n.execution_id, n.artifact_id, extension
                )
            || n.sha256.len() != 64
            || !n.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(Error::Invalid("artifact storage metadata"));
        }
        let mut c = self.0.lock().unwrap();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let count: u64 = tx.query_row(
            "SELECT count(*) FROM artifacts WHERE execution_id=?1",
            [&n.execution_id],
            |r| r.get(0),
        )?;
        if count >= max_per_execution {
            return Err(Error::Limit("artifact count"));
        }
        if let Some(attempt_id) = &n.provider_attempt_id {
            let attempt_execution: String = tx
                .query_row(
                    "SELECT execution_id FROM provider_attempts WHERE provider_attempt_id=?1",
                    [attempt_id],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or(Error::NotFound)?;
            if attempt_execution != n.execution_id {
                return Err(Error::Invalid("artifact attempt relationship"));
            }
        }
        tx.execute(
            "INSERT INTO artifacts(artifact_id,execution_id,provider_attempt_id,kind,storage_backend,storage_key,media_type,size_bytes,sha256,metadata_json,metadata_schema_version,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                n.artifact_id,
                n.execution_id,
                n.provider_attempt_id,
                n.kind,
                n.storage_backend,
                n.storage_key,
                n.media_type,
                n.size_bytes,
                n.sha256,
                j(&n.metadata),
                n.metadata_schema_version,
                n.created_at
            ],
        )?;
        let artifact = Artifact {
            artifact_id: n.artifact_id.clone(),
            execution_id: n.execution_id.clone(),
            provider_attempt_id: n.provider_attempt_id.clone(),
            kind: n.kind.clone(),
            storage_backend: n.storage_backend.clone(),
            media_type: n.media_type.clone(),
            storage_key: n.storage_key.clone(),
            size_bytes: n.size_bytes,
            sha256: n.sha256.clone(),
            metadata: n.metadata.clone(),
            metadata_schema_version: n.metadata_schema_version,
            created_at: n.created_at.clone(),
        };
        tx.commit()?;
        Ok(artifact)
    }
    pub fn count_artifacts_for_execution(&self, execution_id: &str) -> Result<u64> {
        self.0
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM artifacts WHERE execution_id=?1",
                [execution_id],
                |r| r.get(0),
            )
            .map_err(Into::into)
    }
    pub fn count_artifacts_for_attempt(&self, provider_attempt_id: &str) -> Result<u64> {
        self.0
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM artifacts WHERE provider_attempt_id=?1",
                [provider_attempt_id],
                |r| r.get(0),
            )
            .map_err(Into::into)
    }
    pub fn get_artifact_for_account(
        &self,
        artifact_id: &str,
        account_id: &str,
    ) -> Result<Artifact> {
        self.0
            .lock()
            .unwrap()
            .query_row(
                &format!("{ARTIFACT_SELECT} JOIN executions e ON e.execution_id=a.execution_id WHERE a.artifact_id=?1 AND e.account_id=?2"),
                params![artifact_id, account_id],
                map_artifact,
            )
            .optional()?
            .ok_or(Error::NotFound)
    }
    pub fn get_artifact(&self, artifact_id: &str) -> Result<Artifact> {
        self.0
            .lock()
            .unwrap()
            .query_row(
                &format!("{ARTIFACT_SELECT} WHERE a.artifact_id=?1"),
                [artifact_id],
                map_artifact,
            )
            .optional()?
            .ok_or(Error::NotFound)
    }
    pub fn list_artifacts_for_account(
        &self,
        execution_id: &str,
        account_id: &str,
    ) -> Result<Vec<Artifact>> {
        let c = self.0.lock().unwrap();
        let mut statement = c.prepare(&format!("{ARTIFACT_SELECT} JOIN executions e ON e.execution_id=a.execution_id WHERE a.execution_id=?1 AND e.account_id=?2 ORDER BY a.created_at,a.artifact_id"))?;
        let rows = statement.query_map(params![execution_id, account_id], map_artifact)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
    pub fn create_receipt(&self, n: &CreateReceiptParams) -> Result<Receipt> {
        n.actual_vendor_cost
            .validate()
            .map_err(|_| Error::Invalid("actual vendor cost"))?;
        self.reject_registered_secrets([
            n.receipt_id.as_str(),
            n.execution_id.as_str(),
            n.provider_attempt_id.as_str(),
            n.currency.as_str(),
            n.pricing_catalog_version.as_str(),
            n.actual_vendor_cost.currency.as_str(),
            n.created_at.as_str(),
            n.settled_at.as_deref().unwrap_or(""),
            n.hubu_settlement_id.as_deref().unwrap_or(""),
        ])?;
        self.reject_registered_numbers([
            n.settlement_minor,
            n.actual_vendor_cost.amount,
            i64::from(n.actual_vendor_cost.scale),
        ])?;
        if n.settlement_minor < 0 {
            return Err(Error::Invalid("settlement"));
        }
        let attempt = self.get_provider_attempt(&n.provider_attempt_id)?;
        if attempt.execution_id != n.execution_id {
            return Err(Error::Invalid("receipt attempt relationship"));
        }
        if attempt.outcome != "succeeded"
            || attempt.completed_at.is_none()
            || (attempt.provider_request_id.is_none() && attempt.provider_operation_id.is_none())
        {
            return Err(Error::Invalid("receipt requires a succeeded attempt"));
        }
        let c = self.0.lock().unwrap();
        let auth: (i64, String, String) = c
            .query_row(
                "SELECT authorized_minor,authorization_currency,pricing_snapshot_json FROM executions WHERE execution_id=?1",
                [&n.execution_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .ok_or(Error::NotFound)?;
        let price_model_snapshot: Value =
            serde_json::from_str(&auth.2).map_err(|_| Error::Invalid("pricing snapshot"))?;
        let snapshot: PricingSnapshot = serde_json::from_value(price_model_snapshot.clone())
            .map_err(|_| Error::Invalid("pricing snapshot"))?;
        snapshot
            .validate_integrity()
            .map_err(|_| Error::Invalid("pricing snapshot"))?;
        if n.pricing_catalog_version != snapshot.catalog_version {
            return Err(Error::Invalid("pricing catalog version"));
        }
        let usage: NormalizedUsage = serde_json::from_value(
            attempt
                .usage
                .clone()
                .ok_or(Error::Invalid("receipt requires provider usage"))?,
        )
        .map_err(|_| Error::Invalid("provider usage"))?;
        let expected = snapshot
            .settle_precise(&usage, attempt.actual_vendor_cost.as_ref(), auth.0)
            .map_err(|error| match error {
                crate::provider_contract::ContractError::SettlementOverage => {
                    Error::OverAuthorization
                }
                _ => Error::Invalid("precise settlement"),
            })?;
        if expected.budget_amount_minor != n.settlement_minor
            || expected.actual_vendor_cost != n.actual_vendor_cost
            || !n.currency.eq_ignore_ascii_case(&auth.1)
            || !n.currency.eq_ignore_ascii_case(&snapshot.currency)
        {
            return Err(Error::OverAuthorization);
        }
        let provider_request_id = attempt
            .provider_request_id
            .clone()
            .or(attempt.provider_operation_id.clone())
            .ok_or(Error::Invalid("receipt requires provider evidence"))?;
        let pricing_snapshot_json = auth.2;
        c.execute(
            "INSERT INTO receipts(receipt_id,execution_id,provider_attempt_id,settlement_minor,currency,pricing_catalog_version,actual_vendor_cost_amount,actual_vendor_cost_scale,actual_vendor_cost_currency,provider_request_id,pricing_snapshot_json,created_at,settled_at,hubu_settlement_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            params![
                n.receipt_id,
                n.execution_id,
                n.provider_attempt_id,
                n.settlement_minor,
                n.currency,
                n.pricing_catalog_version,
                n.actual_vendor_cost.amount,
                n.actual_vendor_cost.scale,
                n.actual_vendor_cost.currency,
                provider_request_id,
                pricing_snapshot_json,
                n.created_at,
                n.settled_at,
                n.hubu_settlement_id
            ],
        )?;
        Ok(Receipt {
            receipt_id: n.receipt_id.clone(),
            execution_id: n.execution_id.clone(),
            provider_attempt_id: n.provider_attempt_id.clone(),
            settlement_minor: n.settlement_minor,
            currency: n.currency.clone(),
            pricing_catalog_version: n.pricing_catalog_version.clone(),
            actual_vendor_cost: n.actual_vendor_cost.clone(),
            provider_request_id,
            price_model_snapshot,
            created_at: n.created_at.clone(),
            transmission_started_at: None,
            settled_at: n.settled_at.clone(),
            hubu_settlement_id: n.hubu_settlement_id.clone(),
        })
    }
    pub fn get_receipt_for_execution(&self, execution_id: &str) -> Result<Receipt> {
        self.0.lock().unwrap().query_row("SELECT receipt_id,execution_id,provider_attempt_id,settlement_minor,currency,pricing_catalog_version,actual_vendor_cost_amount,actual_vendor_cost_scale,actual_vendor_cost_currency,provider_request_id,pricing_snapshot_json,created_at,transmission_started_at,settled_at,hubu_settlement_id FROM receipts WHERE execution_id=?1",[execution_id],|r| {
            let pricing_snapshot_json:String=r.get(10)?;
            let price_model_snapshot=serde_json::from_str(&pricing_snapshot_json).map_err(|error| rusqlite::Error::FromSqlConversionFailure(pricing_snapshot_json.len(),rusqlite::types::Type::Text,Box::new(error)))?;
            Ok(Receipt { receipt_id:r.get(0)?,execution_id:r.get(1)?,provider_attempt_id:r.get(2)?,settlement_minor:r.get(3)?,currency:r.get(4)?,pricing_catalog_version:r.get(5)?,actual_vendor_cost:actual_vendor_cost_from_row(r,6,7,8)?.ok_or_else(|| rusqlite::Error::InvalidColumnType(6,"actual_vendor_cost_amount".into(),rusqlite::types::Type::Null))?,provider_request_id:r.get(9)?,price_model_snapshot,created_at:r.get(11)?,transmission_started_at:r.get(12)?,settled_at:r.get(13)?,hubu_settlement_id:r.get(14)? })
        }).optional()?.ok_or(Error::NotFound)
    }
    pub fn begin_settlement_transmission(&self, receipt_id: &str, at: &str) -> Result<()> {
        let changed = self.0.lock().unwrap().execute("UPDATE receipts SET transmission_started_at=?1 WHERE receipt_id=?2 AND transmission_started_at IS NULL AND settled_at IS NULL", params![at,receipt_id])?;
        if changed == 1 {
            Ok(())
        } else {
            Err(Error::Stale)
        }
    }
    pub fn complete_receipt(
        &self,
        receipt_id: &str,
        settlement_id: &str,
        settled_at: &str,
    ) -> Result<()> {
        let changed=self.0.lock().unwrap().execute("UPDATE receipts SET settled_at=?1,hubu_settlement_id=?2 WHERE receipt_id=?3 AND settled_at IS NULL",params![settled_at,settlement_id,receipt_id])?;
        if changed == 1 {
            Ok(())
        } else {
            Err(Error::Stale)
        }
    }
    pub fn record_reconciliation(
        &self,
        execution: &Execution,
        last_confirmed_step: &str,
        redacted_error: Option<&str>,
        at: &str,
    ) -> Result<ReconciliationRecord> {
        let attempt = self
            .get_provider_attempt_for_execution(&execution.execution_id)
            .ok();
        let receipt = self.get_receipt_for_execution(&execution.execution_id).ok();
        let artifacts = self.count_artifacts_for_execution(&execution.execution_id)?;
        let provider_outcome_ambiguous = execution.provider_outcome
            == Some(LifecycleOutcome::Ambiguous)
            || attempt.as_ref().is_some_and(|attempt| {
                attempt.outcome == "ambiguous"
                    || (attempt.outcome == "started"
                        && attempt.transmission_started_at.is_some()
                        && attempt.completed_at.is_none())
            });
        let last_confirmed_step = if provider_outcome_ambiguous
            && attempt
                .as_ref()
                .is_some_and(|attempt| attempt.operation_checkpointed_at.is_some())
        {
            "provider_operation_checkpointed"
        } else {
            last_confirmed_step
        };
        let recovery_guidance = provider_outcome_ambiguous.then(|| serde_json::json!({
            "provider_outcome_ambiguous": true,
            "billing_may_have_occurred": true,
            "do_not_resubmit": true,
            "recover_first": true,
            "artifact_recovery_time_sensitive": true,
            "action": if attempt.as_ref().and_then(|a| a.provider_recovery_context.as_ref()).and_then(|context| context.validation_reason.as_deref()) == Some("host_not_allowlisted") { "update_policy_then_reinspect" } else { "contact_provider_support" }
        }));
        let mut evidence = serde_json::json!({
            "execution_id": execution.execution_id,
            "provider": execution.provider,
            "target": execution.target,
            "model": execution.model,
            "provider_attempt_id": attempt.as_ref().map(|a| &a.provider_attempt_id),
            "provider_request_id": attempt.as_ref().and_then(|a| a.provider_request_id.as_ref()),
            "provider_operation_id": attempt.as_ref().and_then(|a| a.provider_operation_id.as_ref()),
            "polling_recovery": attempt.as_ref().and_then(|a| a.provider_recovery_context.as_ref()),
            "timestamps": {"created_at": execution.created_at, "updated_at": at, "started_at": execution.started_at, "attempt_started_at": attempt.as_ref().map(|a| &a.started_at), "transmission_started_at": attempt.as_ref().and_then(|a| a.transmission_started_at.as_ref()), "attempt_completed_at": attempt.as_ref().and_then(|a| a.completed_at.as_ref())},
            "last_confirmed_step": last_confirmed_step,
            "redacted_error": redacted_error.map(|v| self.1.redact(v)),
            "pricing_snapshot": execution.pricing_snapshot,
            "authorization": {"account_id": execution.account_id, "operation_key": execution.operation_key, "spend_auth_token_id": execution.hubu_token_reference.as_str(), "claim_id": execution.hubu_claim_id, "authorized_minor": execution.authorized_minor, "currency": execution.authorization_currency},
            "credential_binding": {"provider_config_version": execution.provider_config_version, "provider_config_digest": execution.provider_config_digest},
            "provider_outcome": attempt.as_ref().map(|a| &a.outcome),
            "actual_vendor_cost": attempt.as_ref().and_then(|a| a.actual_vendor_cost.as_ref()),
            "usage": attempt.as_ref().and_then(|a| a.usage.as_ref()),
            "receipt": receipt.as_ref().map(|r| serde_json::json!({"receipt_id":r.receipt_id,"settlement_minor":r.settlement_minor,"currency":r.currency,"actual_vendor_cost":r.actual_vendor_cost,"provider_request_id":r.provider_request_id,"price_model_snapshot":r.price_model_snapshot,"transmission_started_at":r.transmission_started_at,"settled_at":r.settled_at,"hubu_settlement_id":r.hubu_settlement_id})),
            "artifact_count": artifacts,
            "recovery_guidance": recovery_guidance
        });
        if !provider_outcome_ambiguous {
            evidence
                .as_object_mut()
                .expect("reconciliation evidence is an object")
                .remove("recovery_guidance");
        }
        self.reject_registered_json([&evidence])?;
        let c = self.0.lock().unwrap();
        c.execute("INSERT INTO reconciliation_records(execution_id,evidence_json,evidence_schema_version,last_confirmed_step,entered_at,updated_at) VALUES(?1,?2,3,?3,?4,?4) ON CONFLICT(execution_id) DO UPDATE SET evidence_json=excluded.evidence_json,evidence_schema_version=excluded.evidence_schema_version,last_confirmed_step=excluded.last_confirmed_step,updated_at=excluded.updated_at", params![execution.execution_id,j(&evidence),last_confirmed_step,at])?;
        drop(c);
        self.get_reconciliation(&execution.execution_id)
    }
    pub fn get_reconciliation(&self, execution_id: &str) -> Result<ReconciliationRecord> {
        self.0.lock().unwrap().query_row("SELECT execution_id,evidence_json,last_confirmed_step,entered_at,updated_at,automatic_attempts,last_automatic_attempt_at,automatic_attempts_exhausted,last_operator_action_id,last_operator_action FROM reconciliation_records WHERE execution_id=?1",[execution_id],|r| { let evidence:String=r.get(1)?; Ok(ReconciliationRecord { execution_id:r.get(0)?, evidence:serde_json::from_str(&evidence).map_err(|e| rusqlite::Error::FromSqlConversionFailure(evidence.len(),rusqlite::types::Type::Text,Box::new(e)))?, last_confirmed_step:r.get(2)?,entered_at:r.get(3)?,updated_at:r.get(4)?,automatic_attempts:r.get(5)?,last_automatic_attempt_at:r.get(6)?,automatic_attempts_exhausted:r.get::<_,i64>(7)? != 0,last_operator_action_id:r.get(8)?,last_operator_action:r.get(9)? }) }).optional()?.ok_or(Error::NotFound)
    }
    pub fn mark_recovery_attempt(
        &self,
        execution_id: &str,
        at: &str,
        exhausted: bool,
    ) -> Result<ReconciliationRecord> {
        let changed=self.0.lock().unwrap().execute("UPDATE reconciliation_records SET automatic_attempts=automatic_attempts+1,last_automatic_attempt_at=?1,automatic_attempts_exhausted=?2,updated_at=?1 WHERE execution_id=?3",params![at,i64::from(exhausted),execution_id])?;
        if changed != 1 {
            return Err(Error::NotFound);
        }
        self.get_reconciliation(execution_id)
    }
    pub fn record_operator_action(
        &self,
        execution_id: &str,
        action_id: &str,
        action: &str,
        evidence: &Value,
        at: &str,
    ) -> Result<bool> {
        let redacted_evidence = self.1.redact(&j(evidence));
        let redacted_evidence: Value = serde_json::from_str(&redacted_evidence)
            .map_err(|_| Error::Invalid("operator evidence"))?;
        let mut c = self.0.lock().unwrap();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let inserted=tx.execute("INSERT OR IGNORE INTO reconciliation_operator_actions(execution_id,action_id,action,evidence_json,created_at) VALUES(?1,?2,?3,?4,?5)",params![execution_id,action_id,action,j(&redacted_evidence),at])?;
        if inserted == 0 {
            let existing:(String,String)=tx.query_row("SELECT action,evidence_json FROM reconciliation_operator_actions WHERE execution_id=?1 AND action_id=?2",params![execution_id,action_id],|r|Ok((r.get(0)?,r.get(1)?)))?;
            if existing != (action.to_owned(), j(&redacted_evidence)) {
                return Ok(false);
            }
            return Ok(true);
        }
        let changed=tx.execute("UPDATE reconciliation_records SET last_operator_action_id=?1,last_operator_action=?2,updated_at=?3 WHERE execution_id=?4",params![action_id,action,at,execution_id])?;
        if changed != 1 {
            return Err(Error::NotFound);
        }
        tx.commit()?;
        Ok(true)
    }
    #[cfg(test)]
    fn count(&self, t: &str) -> i64 {
        self.0
            .lock()
            .unwrap()
            .query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0))
            .unwrap()
    }
    #[cfg(test)]
    fn delete(&self, id: &str) -> rusqlite::Result<usize> {
        self.0
            .lock()
            .unwrap()
            .execute("DELETE FROM executions WHERE execution_id=?1", [id])
    }
    fn reject_registered_secrets<'a>(
        &self,
        values: impl IntoIterator<Item = &'a str>,
    ) -> Result<()> {
        if values
            .into_iter()
            .any(|value| self.1.contains_registered_secret(value))
        {
            Err(Error::Invalid("secret-bearing persistence value"))
        } else {
            Ok(())
        }
    }
    fn reject_registered_json<'a>(
        &self,
        values: impl IntoIterator<Item = &'a Value>,
    ) -> Result<()> {
        if values
            .into_iter()
            .any(|value| self.1.json_contains_registered_secret(value))
        {
            Err(Error::Invalid("secret-bearing persistence value"))
        } else {
            Ok(())
        }
    }
    fn reject_registered_numbers(&self, values: impl IntoIterator<Item = i64>) -> Result<()> {
        if values
            .into_iter()
            .any(|value| self.1.contains_registered_secret(&value.to_string()))
        {
            Err(Error::Invalid("secret-bearing persistence value"))
        } else {
            Ok(())
        }
    }
}
fn migrate_artifact_storage_columns(c: &Connection) -> rusqlite::Result<()> {
    let artifact_columns = || -> rusqlite::Result<std::collections::BTreeSet<String>> {
        let mut statement = c.prepare("PRAGMA table_info(artifacts)")?;
        let columns = statement
            .query_map([], |row| row.get(1))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(columns)
    };

    let mut existing = artifact_columns()?;
    if !existing.contains("storage_backend") {
        c.execute(
            "ALTER TABLE artifacts ADD COLUMN storage_backend TEXT NOT NULL DEFAULT 'local_fs'",
            [],
        )?;
        existing.insert("storage_backend".to_string());
    }
    if existing.contains("byte_size") && !existing.contains("size_bytes") {
        c.execute(
            "ALTER TABLE artifacts RENAME COLUMN byte_size TO size_bytes",
            [],
        )?;
    }
    Ok(())
}
fn migrate_legacy_authorization_aliases(c: &Connection) -> rusqlite::Result<()> {
    // These original v1 columns both meant the opaque Hubu spend-auth token
    // identifier. A short-lived pre-HUB-70 build wrote decision IDs into the
    // misleadingly named column; the separately persisted snapshot remains the
    // sole authoritative decision-ID record.
    c.execute(
        "UPDATE executions SET hubu_authorization_id=hubu_token_reference WHERE hubu_authorization_id<>hubu_token_reference",
        [],
    )?;
    Ok(())
}
fn migrate_resolved_target_columns(c: &Connection) -> rusqlite::Result<()> {
    let mut statement = c.prepare("PRAGMA table_info(executions)")?;
    let existing: std::collections::BTreeSet<String> = statement
        .query_map([], |row| row.get(1))?
        .collect::<rusqlite::Result<_>>()?;
    for column in [
        "workload_type",
        "provider",
        "adapter",
        "model",
        "provider_config_version",
        "provider_config_digest",
    ] {
        if !existing.contains(column) {
            c.execute(
                &format!("ALTER TABLE executions ADD COLUMN {column} TEXT NOT NULL DEFAULT 'legacy-unresolved' CHECK(trim({column})<>'')"),
                [],
            )?;
        }
    }
    Ok(())
}
fn migrate_execution_scope_column(c: &Connection) -> rusqlite::Result<()> {
    let mut statement = c.prepare("PRAGMA table_info(executions)")?;
    let existing: std::collections::BTreeSet<String> = statement
        .query_map([], |row| row.get(1))?
        .collect::<rusqlite::Result<_>>()?;
    if !existing.contains("execution_scope_json") {
        c.execute(
            "ALTER TABLE executions ADD COLUMN execution_scope_json TEXT CHECK(execution_scope_json IS NULL OR json_valid(execution_scope_json))",
            [],
        )?;
    }
    Ok(())
}
fn migrate_precise_vendor_cost_columns(c: &Connection) -> rusqlite::Result<()> {
    fn columns(
        c: &Connection,
        table: &str,
    ) -> rusqlite::Result<std::collections::BTreeSet<String>> {
        let mut statement = c.prepare(&format!("PRAGMA table_info({table})"))?;
        let collected = statement
            .query_map([], |row| row.get(1))?
            .collect::<rusqlite::Result<_>>();
        collected
    }

    let attempt_columns = columns(c, "provider_attempts")?;
    for (name, definition) in [
        ("actual_vendor_cost_amount", "INTEGER"),
        ("actual_vendor_cost_scale", "INTEGER"),
        ("actual_vendor_cost_currency", "TEXT"),
    ] {
        if !attempt_columns.contains(name) {
            c.execute(
                &format!("ALTER TABLE provider_attempts ADD COLUMN {name} {definition}"),
                [],
            )?;
        }
    }
    // v4 stored provider amounts in currency minor units. That legacy value is
    // exact at decimal scale 2 and remains a valid deterministic backfill.
    c.execute(
        "UPDATE provider_attempts SET actual_vendor_cost_amount=provider_amount_minor,actual_vendor_cost_scale=2,actual_vendor_cost_currency=upper(provider_currency) WHERE actual_vendor_cost_amount IS NULL AND provider_amount_minor IS NOT NULL AND provider_currency IS NOT NULL",
        [],
    )?;

    let receipt_columns = columns(c, "receipts")?;
    for (name, definition) in [
        ("actual_vendor_cost_amount", "INTEGER"),
        ("actual_vendor_cost_scale", "INTEGER"),
        ("actual_vendor_cost_currency", "TEXT"),
        ("provider_request_id", "TEXT"),
        ("pricing_snapshot_json", "TEXT"),
    ] {
        if !receipt_columns.contains(name) {
            c.execute(
                &format!("ALTER TABLE receipts ADD COLUMN {name} {definition}"),
                [],
            )?;
        }
    }
    c.execute(
        "UPDATE receipts SET actual_vendor_cost_amount=settlement_minor,actual_vendor_cost_scale=2,actual_vendor_cost_currency=upper(currency) WHERE actual_vendor_cost_amount IS NULL",
        [],
    )?;
    // Before precise receipts, Gongbu deliberately used its deterministic
    // receipt ID as the provider request reference on the Hubu wire. Preserve
    // that exact value so a lost-response retry still compares equal in Hubu.
    c.execute(
        "UPDATE receipts SET provider_request_id=receipt_id WHERE provider_request_id IS NULL",
        [],
    )?;
    // The legacy Hubu wire carried a reduced price-model object rather than the
    // complete Gongbu pricing snapshot. Reconstruct that exact JSON value for
    // already-created receipts; new receipts persist the full snapshot when
    // they are created.
    c.execute(
        "UPDATE receipts
         SET pricing_snapshot_json=(
             SELECT json_object(
                 'provider', executions.provider,
                 'model', executions.model,
                 'unit_price_cents', json_extract(executions.pricing_snapshot_json, '$.estimated_amount_minor'),
                 'pricing_unit', 'execution',
                 'currency', lower(executions.authorization_currency)
             )
             FROM executions
             WHERE executions.execution_id=receipts.execution_id
         )
         WHERE pricing_snapshot_json IS NULL",
        [],
    )?;
    Ok(())
}

fn migrate_provider_operation_checkpoint_columns(c: &Connection) -> rusqlite::Result<()> {
    let mut statement = c.prepare("PRAGMA table_info(provider_attempts)")?;
    let existing: std::collections::BTreeSet<String> = statement
        .query_map([], |row| row.get(1))?
        .collect::<rusqlite::Result<_>>()?;
    drop(statement);
    for (name, definition) in [
        ("provider_polling_host", "TEXT"),
        ("provider_deadline_unix_ms", "INTEGER"),
        ("operation_checkpointed_at", "TEXT"),
        ("provider_recovery_context_json", "TEXT"),
    ] {
        if !existing.contains(name) {
            c.execute(
                &format!("ALTER TABLE provider_attempts ADD COLUMN {name} {definition}"),
                [],
            )?;
        }
    }
    Ok(())
}

fn migrate_provider_transport_counter_columns(c: &Connection) -> rusqlite::Result<()> {
    let mut statement = c.prepare("PRAGMA table_info(provider_attempts)")?;
    let existing: std::collections::BTreeSet<String> = statement
        .query_map([], |row| row.get(1))?
        .collect::<rusqlite::Result<_>>()?;
    drop(statement);
    for name in ["provider_poll_count", "artifact_fetch_count"] {
        if !existing.contains(name) {
            c.execute(
                &format!(
                    "ALTER TABLE provider_attempts ADD COLUMN {name} INTEGER NOT NULL DEFAULT 0 CHECK({name}>=0)"
                ),
                [],
            )?;
        }
    }
    Ok(())
}

fn actual_vendor_cost_from_row(
    row: &rusqlite::Row<'_>,
    amount_index: usize,
    scale_index: usize,
    currency_index: usize,
) -> rusqlite::Result<Option<ActualVendorCost>> {
    let amount: Option<i64> = row.get(amount_index)?;
    let scale: Option<i64> = row.get(scale_index)?;
    let currency: Option<String> = row.get(currency_index)?;
    match (amount, scale, currency) {
        (None, None, None) => Ok(None),
        (Some(amount), Some(scale), Some(currency)) => {
            let scale = u32::try_from(scale).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    scale_index,
                    rusqlite::types::Type::Integer,
                    Box::new(error),
                )
            })?;
            ActualVendorCost::new(amount, scale, currency)
                .map(Some)
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        amount_index,
                        rusqlite::types::Type::Integer,
                        Box::new(error),
                    )
                })
        }
        _ => Err(rusqlite::Error::InvalidColumnType(
            amount_index,
            "actual_vendor_cost".into(),
            rusqlite::types::Type::Null,
        )),
    }
}
const ARTIFACT_SELECT: &str = "SELECT a.artifact_id,a.execution_id,a.provider_attempt_id,a.kind,a.storage_backend,a.storage_key,a.media_type,a.size_bytes,a.sha256,a.metadata_json,a.metadata_schema_version,a.created_at FROM artifacts a";
fn map_artifact(r: &rusqlite::Row) -> rusqlite::Result<Artifact> {
    let metadata: String = r.get(9)?;
    Ok(Artifact {
        artifact_id: r.get(0)?,
        execution_id: r.get(1)?,
        provider_attempt_id: r.get(2)?,
        kind: r.get(3)?,
        storage_backend: r.get(4)?,
        storage_key: r.get(5)?,
        media_type: r.get(6)?,
        size_bytes: r.get(7)?,
        sha256: r.get(8)?,
        metadata: serde_json::from_str(&metadata).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                metadata.len(),
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })?,
        metadata_schema_version: r.get(10)?,
        created_at: r.get(11)?,
    })
}
fn query_key(c: &Connection, a: &str, k: &str) -> Result<Execution> {
    c.query_row(
        &format!("{EXECUTION_SELECT} WHERE account_id=?1 AND operation_key=?2"),
        params![a, k],
        map,
    )
    .map_err(Into::into)
}
fn query_id(c: &Connection, id: &str) -> Result<Execution> {
    c.query_row(
        &format!("{EXECUTION_SELECT} WHERE execution_id=?1"),
        [id],
        map,
    )
    .optional()?
    .ok_or(Error::NotFound)
}
fn query_token(c: &Connection, spend_auth_token_id: &str) -> Result<Execution> {
    if let Some(execution) = c
        .query_row(
        &format!(
            "{EXECUTION_SELECT} WHERE execution_id=(SELECT execution_id FROM hubu_authorization_snapshots WHERE spend_auth_token_id=?1)"
        ),
        [spend_auth_token_id],
        map,
    )
        .optional()?
    {
        return Ok(execution);
    }

    let legacy_execution_ids = {
        let mut statement = c.prepare(
            "SELECT executions.execution_id
             FROM executions
             LEFT JOIN hubu_authorization_snapshots
               ON hubu_authorization_snapshots.execution_id=executions.execution_id
             WHERE executions.hubu_token_reference=?1
               AND hubu_authorization_snapshots.execution_id IS NULL
             ORDER BY executions.execution_id
             LIMIT 2",
        )?;
        let execution_ids = statement
            .query_map([spend_auth_token_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        execution_ids
    };
    match legacy_execution_ids.as_slice() {
        [] => Err(Error::NotFound),
        [execution_id] => query_id(c, execution_id),
        _ => Err(Error::AmbiguousLegacyToken),
    }
}
const EXECUTION_SELECT: &str = "SELECT execution_id,account_id,operation_key,hubu_authorization_id,hubu_claim_id,hubu_token_reference,authorized_minor,authorization_currency,normalized_input_json,input_hash,input_schema_version,target,config_version,workload_type,provider,adapter,model,provider_config_version,provider_config_digest,pricing_snapshot_json,pricing_schema_version,status,outcome,provider_outcome,artifact_outcome,settlement_outcome,failure_code,failure_message_redacted,created_at,updated_at,started_at,completed_at,release_transmission_started_at,version,execution_scope_json FROM executions";
fn map(r: &rusqlite::Row) -> rusqlite::Result<Execution> {
    let i: String = r.get(8)?;
    let p: String = r.get(19)?;
    let execution_scope_json: Option<String> = r.get(34)?;
    Ok(Execution {
        execution_id: r.get(0)?,
        account_id: r.get(1)?,
        operation_key: r.get(2)?,
        hubu_authorization_id: r.get(3)?,
        hubu_claim_id: r.get(4)?,
        hubu_token_reference: HubuTokenReference(r.get(5)?),
        authorized_minor: r.get(6)?,
        authorization_currency: r.get(7)?,
        normalized_input: serde_json::from_str(&i).unwrap(),
        input_hash: r.get(9)?,
        input_schema_version: r.get(10)?,
        target: r.get(11)?,
        config_version: r.get(12)?,
        workload_type: r.get(13)?,
        provider: r.get(14)?,
        adapter: r.get(15)?,
        model: r.get(16)?,
        provider_config_version: r.get(17)?,
        provider_config_digest: r.get(18)?,
        pricing_snapshot: serde_json::from_str(&p).unwrap(),
        pricing_schema_version: r.get(20)?,
        execution_scope: execution_scope_json
            .map(|json| {
                serde_json::from_str(&json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        json.len(),
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
            })
            .transpose()?,
        status: r.get(21)?,
        outcome: r.get(22)?,
        provider_outcome: map_lifecycle_outcome(r.get(23)?)?,
        artifact_outcome: map_lifecycle_outcome(r.get(24)?)?,
        settlement_outcome: map_lifecycle_outcome(r.get(25)?)?,
        failure_code: r.get(26)?,
        failure_message_redacted: r.get(27)?,
        created_at: r.get(28)?,
        updated_at: r.get(29)?,
        started_at: r.get(30)?,
        completed_at: r.get(31)?,
        release_transmission_started_at: r.get(32)?,
        version: r.get(33)?,
    })
}

fn map_lifecycle_outcome(value: Option<String>) -> rusqlite::Result<Option<LifecycleOutcome>> {
    value
        .map(|value| LifecycleOutcome::parse(&value).ok_or(rusqlite::Error::InvalidQuery))
        .transpose()
}
fn validate_execution(n: &CreateExecutionParams) -> Result<()> {
    if n.account_id.trim().is_empty()
        || n.operation_key.trim().is_empty()
        || n.hubu_authorization_id != n.hubu_token_reference.as_str()
        || n.authorized_minor < 0
        || n.input_schema_version < 1
        || n.pricing_schema_version != PRICING_SNAPSHOT_SCHEMA_VERSION
        || [
            &n.workload_type,
            &n.provider,
            &n.adapter,
            &n.model,
            &n.provider_config_version,
            &n.provider_config_digest,
        ]
        .iter()
        .any(|value| value.trim().is_empty())
    {
        return Err(Error::Invalid("execution"));
    }
    let digest = n.provider_config_digest.strip_prefix("sha256:");
    if digest.is_none_or(|value| {
        value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        return Err(Error::Invalid("provider config digest"));
    }
    safe_json(&n.normalized_input)?;
    safe_json(&n.pricing_snapshot)?;
    if n.execution_scope
        .as_ref()
        .is_some_and(|scope| for_target(&n.provider, &n.adapter).as_ref() != Some(scope))
    {
        return Err(Error::Invalid("execution scope"));
    }
    let snapshot: PricingSnapshot = serde_json::from_value(n.pricing_snapshot.clone())
        .map_err(|_| Error::Invalid("pricing snapshot"))?;
    if i64::from(snapshot.schema_version) != n.pricing_schema_version {
        return Err(Error::Invalid("pricing schema version"));
    }
    snapshot
        .check_authorization(n.authorized_minor, &n.authorization_currency)
        .map_err(|_| Error::Invalid("pricing snapshot authorization"))?;
    if snapshot.provider != n.provider || snapshot.model != n.model {
        return Err(Error::Invalid("pricing snapshot target"));
    }
    Ok(())
}
fn safe_json(v: &Value) -> Result<()> {
    const BAD: [&str; 7] = [
        "token",
        "authorization",
        "api_key",
        "apikey",
        "secret",
        "password",
        "credential",
    ];
    fn walk(v: &Value) -> bool {
        match v {
            Value::Object(m) => m
                .iter()
                .any(|(k, v)| BAD.iter().any(|b| k.to_ascii_lowercase().contains(b)) || walk(v)),
            Value::Array(a) => a.iter().any(walk),
            _ => false,
        }
    }
    if walk(v) {
        Err(Error::Invalid("secret-bearing JSON"))
    } else {
        Ok(())
    }
}

fn safe_usage_json(value: &Value) -> Result<()> {
    let _: crate::provider_contract::NormalizedUsage =
        serde_json::from_value(value.clone()).map_err(|_| Error::Invalid("provider usage"))?;
    Ok(())
}
fn status(s: &str) -> Result<()> {
    if [
        "pending",
        "preflighting",
        "claimed",
        "executing",
        "settling",
        "succeeded",
        "failed",
        "released",
        "reconciliation_required",
    ]
    .contains(&s)
    {
        Ok(())
    } else {
        Err(Error::Invalid("status"))
    }
}
fn allowed_transition(from: &str, to: &str) -> bool {
    if matches!(from, "succeeded" | "released" | "failed") {
        return false;
    }
    matches!(
        (from, to),
        ("pending", "preflighting")
            | ("pending", "failed")
            | ("preflighting", "claimed")
            | ("preflighting", "failed")
            | ("preflighting", "reconciliation_required")
            | ("claimed", "executing")
            | ("claimed", "released")
            | ("claimed", "reconciliation_required")
            | ("executing", "settling")
            | ("executing", "released")
            | ("executing", "reconciliation_required")
            | ("settling", "succeeded")
            | ("settling", "reconciliation_required")
            | ("reconciliation_required", "succeeded")
            | ("reconciliation_required", "released")
            | ("reconciliation_required", "settling")
    )
}
fn j(v: &Value) -> String {
    serde_json::to_string(v).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        collections::BTreeSet,
        io::{Read, Write},
        net::TcpListener,
        sync::Barrier,
        thread,
    };
    use tempfile::tempdir;
    fn new(a: &str, k: &str) -> CreateExecutionParams {
        CreateExecutionParams {
            account_id: a.into(),
            operation_key: k.into(),
            hubu_authorization_id: "sha256:abc".into(),
            hubu_claim_id: Some("claim".into()),
            hubu_token_reference: HubuTokenReference::new("sha256:abc").unwrap(),
            authorized_minor: 500,
            authorization_currency: "USD".into(),
            normalized_input: json!({"prompt":"cat"}),
            input_hash: "sha256:input".into(),
            input_schema_version: 1,
            target: "mock/image".into(),
            config_version: "cfg-v1".into(),
            workload_type: "image_generation".into(),
            provider: "example".into(),
            adapter: "mock".into(),
            model: "image-v1".into(),
            provider_config_version: "pcv-1".into(),
            provider_config_digest: format!("sha256:{}", "a".repeat(64)),
            pricing_snapshot: json!({
                "schema_version":2,
                "provider":"example","model":"image-v1",
                "catalog_version":"prices-v2","catalog_digest":format!("sha256:{}", "a".repeat(64)),
                "pricing_rule_id":"example-image","components":[{
                    "unit":"image","rate_numerator_minor":100,"rate_denominator":1,"quantity":1
                }],
                "exact_estimate_numerator":"100","exact_estimate_denominator":"1",
                "estimated_amount_minor":100,"currency":"USD"
            }),
            pricing_schema_version: 2,
            execution_scope: None,
            created_at: "2026-08-05T20:00:00Z".into(),
        }
    }
    fn attempt(repo: &Repository, e: &Execution) -> String {
        repo.create_provider_attempt(&CreateProviderAttemptParams {
            execution_id: e.execution_id.clone(),
            provider: "mock".into(),
            provider_request_id: None,
            provider_operation_id: None,
            started_at: "2026-08-05T20:01:00Z".into(),
        })
        .unwrap()
        .provider_attempt_id
    }

    fn submitted_claim(execution: &Execution) -> Value {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut raw = String::new();
            stream.read_to_string(&mut raw).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 422 Unprocessable Entity\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            let body = raw.split_once("\r\n\r\n").unwrap().1;
            serde_json::from_str(body).unwrap()
        });
        let root = tempdir().unwrap();
        let repository =
            Repository::open(root.path().join("hubu.sqlite3"), Redactor::default()).unwrap();
        let hubu = crate::hubu::ProductionHubuActivities::new(
            crate::hubu::HubuClient::new(format!("http://{address}")),
            repository,
        );
        crate::workflow::HubuActivities::claim(&hubu, execution)
            .expect_err("capture server rejects after receiving the claim");
        server.join().unwrap()
    }
    fn complete_success(repo: &Repository, attempt_id: &str) {
        repo.complete_provider_attempt(
            attempt_id,
            &AttemptResult {
                outcome: "succeeded".into(),
                completed_at: "2026-08-05T20:02:00Z".into(),
                usage: json!({"images":1}),
                usage_schema_version: 1,
                actual_vendor_cost: Some(ActualVendorCost::new(100, 2, "USD").unwrap()),
                failure_code: None,
                failure_message_redacted: None,
                provider_request_id: Some("provider-request".into()),
                provider_operation_id: None,
            },
        )
        .unwrap();
    }
    #[test]
    fn scoped_idempotency_and_immutable_snapshots() {
        let r = Repository::in_memory().unwrap();
        let a = r.create_execution(&new("a", "same")).unwrap();
        assert_eq!(a.workload_type, "image_generation");
        assert_eq!(a.provider, "example");
        assert_eq!(a.adapter, "mock");
        assert_eq!(a.model, "image-v1");
        assert_eq!(a.provider_config_version, "pcv-1");
        let mut changed = new("a", "same");
        changed.normalized_input = json!({"prompt":"changed"});
        changed.pricing_snapshot["components"][0]["rate_numerator_minor"] = json!(499);
        changed.pricing_snapshot["exact_estimate_numerator"] = json!(499.to_string());
        changed.pricing_snapshot["estimated_amount_minor"] = json!(499);
        let b = r.create_execution(&changed).unwrap();
        let c = r.create_execution(&new("b", "same")).unwrap();
        assert_eq!(a.execution_id, b.execution_id);
        assert_eq!(a.normalized_input, b.normalized_input);
        assert_eq!(a.pricing_snapshot, b.pricing_snapshot);
        assert_ne!(a.execution_id, c.execution_id)
    }

    #[test]
    fn restart_scan_returns_only_stable_nonterminal_execution_ids() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("gongbu.sqlite3");
        let repository = Repository::open(&path, Redactor::default()).unwrap();
        let pending = repository
            .create_execution(&new("account", "pending"))
            .unwrap();
        let failed = repository
            .create_execution(&new("account", "failed"))
            .unwrap();
        repository
            .update_execution(
                &failed.execution_id,
                failed.version,
                &ExecutionUpdate {
                    status: "failed".into(),
                    outcome: Some("rejected".into()),
                    started_at: None,
                    completed_at: Some("2026-08-05T20:01:00Z".into()),
                    failure_code: Some("preflight_failed".into()),
                    failure_message_redacted: Some("preflight failed".into()),
                    provider_outcome: None,
                    artifact_outcome: None,
                    settlement_outcome: None,
                },
                "2026-08-05T20:01:00Z",
            )
            .unwrap();
        drop(repository);

        let restarted = Repository::open(&path, Redactor::default()).unwrap();
        assert_eq!(
            restarted.list_nonterminal_execution_ids().unwrap(),
            vec![pending.execution_id]
        );
    }

    #[test]
    fn migration_collapses_short_lived_decision_value_without_losing_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("hub-72-layout.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(MIGRATION).unwrap();
        let digest = format!("sha256:{}", "a".repeat(64));
        connection.execute(
            "INSERT INTO executions(execution_id,account_id,operation_key,hubu_authorization_id,hubu_token_reference,authorized_minor,authorization_currency,normalized_input_json,input_hash,input_schema_version,target,config_version,provider_config_digest,pricing_snapshot_json,pricing_schema_version,execution_scope_json,status,created_at,updated_at,version) VALUES('hub-72-row','account-a','operation','decision-1','token-1',100,'USD','{}','sha256:input',1,'image_generation/example/fixture/image-v1','provider-v1',?1,'{}',1,'{\"schema_version\":1,\"provider\":{\"id\":\"provider:local:fixture\",\"display_name\":\"Local fixture provider\"},\"executor\":{\"id\":\"executor:gongbu:image\",\"display_name\":\"Gongbu image executor\"},\"capability\":{\"id\":\"capability:image:generate\",\"display_name\":\"Generate image\"},\"billing_merchant\":{\"id\":\"merchant:local\",\"display_name\":\"Local merchant\"}}','pending','now','now',0)",
            [&digest],
        ).unwrap();
        connection.execute(
            "INSERT INTO hubu_authorization_snapshots(execution_id,account_id,agent_id,operation_key,decision_id,spend_auth_token_id,amount_minor,currency,execution_scope_json,lease_profile,expires_at,authorization_status,reason) VALUES('hub-72-row','account-a','agent-a','operation','decision-1','token-1',100,'USD','{\"schema_version\":1,\"provider\":{\"id\":\"provider:local:fixture\",\"display_name\":\"Local fixture provider\"},\"executor\":{\"id\":\"executor:gongbu:image\",\"display_name\":\"Gongbu image executor\"},\"capability\":{\"id\":\"capability:image:generate\",\"display_name\":\"Generate image\"},\"billing_merchant\":{\"id\":\"merchant:local\",\"display_name\":\"Local merchant\"}}','default','2026-08-05T21:00:00Z','available','migration fixture')",
            [],
        ).unwrap();
        drop(connection);

        let repository = Repository::open(&path, Redactor::default()).unwrap();
        let execution = repository.get_execution("hub-72-row").unwrap();
        assert_eq!(execution.hubu_authorization_id, "token-1");
        assert_eq!(execution.hubu_token_reference.as_str(), "token-1");
        let snapshot = repository
            .get_hubu_authorization_snapshot("hub-72-row")
            .unwrap();
        assert_eq!(snapshot.decision_id, "decision-1");
        assert_eq!(snapshot.spend_auth_token_id, "token-1");
    }

    #[test]
    fn exact_pre_snapshot_database_migrates_replays_and_preserves_reconciliation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pre-hub-70.sqlite3");
        let legacy = Connection::open(&path).unwrap();
        legacy
            .execute_batch(include_str!(
                "../../../../fixtures/gongbu-pre-authorization-snapshot.sql"
            ))
            .unwrap();
        assert_eq!(
            legacy
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='hubu_authorization_snapshots'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        legacy.execute("INSERT INTO provider_attempts(provider_attempt_id,execution_id,provider,provider_request_id,outcome,usage_json,usage_schema_version,provider_amount_minor,provider_currency,started_at,transmission_started_at,completed_at) VALUES('legacy-attempt','legacy-reconciliation','example','legacy-request','succeeded','{\"images\":1}',1,7,'usd','2026-08-05T20:00:10Z','2026-08-05T20:00:10Z','2026-08-05T20:00:20Z')", []).unwrap();
        legacy.execute("INSERT INTO receipts(receipt_id,execution_id,provider_attempt_id,settlement_minor,currency,pricing_catalog_version,created_at) VALUES('legacy-receipt','legacy-reconciliation','legacy-attempt',7,'usd','prices-v2','2026-08-05T20:00:30Z')", []).unwrap();
        drop(legacy);

        let repository = Repository::open(&path, Redactor::default()).unwrap();
        assert_eq!(repository.count("hubu_authorization_snapshots"), 0);
        let replay = repository
            .get_execution_by_hubu_token("account-a", "legacy-token")
            .unwrap();
        assert_eq!(replay.execution_id, "legacy-reconciliation");
        assert_eq!(
            repository
                .get_execution_by_spend_auth_token("legacy-token")
                .unwrap()
                .execution_id,
            "legacy-reconciliation"
        );
        let reconciliation = repository
            .get_reconciliation("legacy-reconciliation")
            .unwrap();
        assert_eq!(
            reconciliation.evidence["provider_request_id"],
            "provider-before-upgrade"
        );
        assert_eq!(reconciliation.automatic_attempts, 2);
        assert!(reconciliation.automatic_attempts_exhausted);
        let legacy_attempt = repository.get_provider_attempt("legacy-attempt").unwrap();
        assert_eq!(
            legacy_attempt.actual_vendor_cost,
            Some(ActualVendorCost::new(7, 2, "USD").unwrap())
        );
        assert_eq!(legacy_attempt.provider_poll_count, 0);
        assert_eq!(legacy_attempt.artifact_fetch_count, 0);
        let legacy_receipt = repository
            .get_receipt_for_execution("legacy-reconciliation")
            .unwrap();
        assert_eq!(
            legacy_receipt.actual_vendor_cost,
            ActualVendorCost::new(7, 2, "USD").unwrap()
        );
        assert_eq!(legacy_receipt.provider_request_id, "legacy-receipt");
        assert_eq!(
            legacy_receipt.price_model_snapshot,
            json!({
                "provider": "example",
                "model": "image-v1",
                "unit_price_cents": 100,
                "pricing_unit": "execution",
                "currency": "usd"
            })
        );
        assert!(repository
            .record_operator_action(
                "legacy-reconciliation",
                "operator-confirmed",
                "release",
                &json!({"review":"legacy evidence retained"}),
                "2026-08-05T20:02:00Z",
            )
            .unwrap());
        drop(repository);

        let restarted = Repository::open(&path, Redactor::default()).unwrap();
        let replay_receipt = restarted
            .get_receipt_for_execution("legacy-reconciliation")
            .unwrap();
        assert_eq!(replay_receipt, legacy_receipt);
        let preserved = restarted
            .get_reconciliation("legacy-reconciliation")
            .unwrap();
        assert_eq!(
            preserved.evidence["provider_request_id"],
            "provider-before-upgrade"
        );
        assert_eq!(
            preserved.last_operator_action_id.as_deref(),
            Some("operator-confirmed")
        );
        assert_eq!(preserved.last_operator_action.as_deref(), Some("release"));
    }

    #[test]
    fn ambiguous_pre_snapshot_token_reference_fails_closed() {
        let repository = Repository::in_memory().unwrap();
        let mut first = new("account-a", "operation-a");
        first.hubu_authorization_id = "legacy-token-a".into();
        first.hubu_token_reference = HubuTokenReference::new("legacy-token-a").unwrap();
        repository.create_execution(&first).unwrap();
        let mut second = new("account-b", "operation-b");
        second.hubu_authorization_id = "legacy-token-b".into();
        second.hubu_token_reference = HubuTokenReference::new("legacy-token-b").unwrap();
        repository.create_execution(&second).unwrap();
        repository
            .0
            .lock()
            .unwrap()
            .execute(
                "UPDATE executions
                 SET hubu_authorization_id='ambiguous-legacy-token',
                     hubu_token_reference='ambiguous-legacy-token'",
                [],
            )
            .unwrap();

        assert!(matches!(
            repository.get_execution_by_spend_auth_token("ambiguous-legacy-token"),
            Err(Error::AmbiguousLegacyToken)
        ));
    }

    #[test]
    fn legacy_database_preserves_claim_contract_and_new_rows_use_typed_scope() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy.sqlite3");
        let legacy = MIGRATION.replace(
            ", provider_config_digest TEXT NOT NULL CHECK(provider_config_digest GLOB 'sha256:*' AND length(provider_config_digest)=71)",
            "",
        ).replace(
            " execution_scope_json TEXT CHECK(execution_scope_json IS NULL OR json_valid(execution_scope_json)),\n",
            "",
        );
        let legacy_connection = Connection::open(&path).unwrap();
        legacy_connection.execute_batch(&legacy).unwrap();
        for column in [
            "workload_type",
            "provider",
            "adapter",
            "model",
            "provider_config_version",
        ] {
            legacy_connection
                .execute(
                    &format!("ALTER TABLE executions ADD COLUMN {column} TEXT NOT NULL DEFAULT 'legacy-unresolved' CHECK(trim({column})<>'')"),
                    [],
                )
                .unwrap();
        }
        legacy_connection.execute(
            "INSERT INTO executions(execution_id,account_id,operation_key,hubu_authorization_id,hubu_token_reference,authorized_minor,authorization_currency,normalized_input_json,input_hash,input_schema_version,target,config_version,workload_type,provider,adapter,model,provider_config_version,pricing_snapshot_json,pricing_schema_version,status,created_at,updated_at,version) VALUES('legacy-execution','legacy','in-flight','auth','token-ref',100,'USD','{}','hash',1,'image_generation/example/fixture/image-v1','pcv-1','image_generation','example','fixture','image-v1','pcv-1','{}',1,'pending','now','now',0)",
            [],
        ).unwrap();
        drop(legacy_connection);

        let repository = Repository::open(&path, Redactor::default()).unwrap();
        let columns: BTreeSet<String> = repository
            .0
            .lock()
            .unwrap()
            .prepare("PRAGMA table_info(executions)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(columns.contains("provider_config_digest"));
        assert!(columns.contains("execution_scope_json"));
        let legacy_execution = repository.get_execution("legacy-execution").unwrap();
        assert_eq!(
            legacy_execution.provider_config_digest,
            crate::provider_targets::LEGACY_UNRESOLVED_DIGEST
        );
        assert_eq!(legacy_execution.execution_scope, None);
        let legacy_claim = submitted_claim(&legacy_execution);
        assert_eq!(legacy_claim["merchant"], "gongbu.execution");
        assert!(legacy_claim.get("execution_scope").is_none());

        let mut omitted = new("migration", "omitted-scope");
        omitted.provider = "google".into();
        omitted.adapter = "gemini_developer_image".into();
        omitted.pricing_snapshot["provider"] = json!("google");
        let omitted = repository.create_execution(&omitted).unwrap();
        assert_eq!(omitted.execution_scope, None);
        let omitted_claim = submitted_claim(&omitted);
        assert_eq!(omitted_claim["merchant"], "gongbu.execution");
        assert!(omitted_claim.get("execution_scope").is_none());

        let expected_scope = for_target("google", "gemini_developer_image").unwrap();
        let mut typed = new("migration", "typed-scope");
        typed.provider = "google".into();
        typed.adapter = "gemini_developer_image".into();
        typed.pricing_snapshot["provider"] = json!("google");
        typed.execution_scope = Some(expected_scope.clone());
        let created = repository.create_execution(&typed).unwrap();
        assert_eq!(
            created.provider_config_digest,
            format!("sha256:{}", "a".repeat(64))
        );
        assert_eq!(created.execution_scope.as_ref(), Some(&expected_scope));
        let typed_claim = submitted_claim(&created);
        assert!(typed_claim.get("merchant").is_none());
        assert_eq!(
            typed_claim["execution_scope"],
            json!(expected_scope.clone())
        );
        let mut broadened = typed;
        broadened.operation_key = "broadened-scope".into();
        broadened.execution_scope.as_mut().unwrap().provider.id = "provider:other".into();
        assert_eq!(
            repository
                .create_execution(&broadened)
                .unwrap_err()
                .to_string(),
            "invalid execution scope"
        );
        drop(repository);
        let restarted = Repository::open(&path, Redactor::default()).unwrap();
        let restarted_execution = restarted.get_execution(&created.execution_id).unwrap();
        assert_eq!(
            restarted_execution.provider_config_digest,
            created.provider_config_digest
        );
        assert_eq!(restarted_execution.execution_scope, Some(expected_scope));
    }
    #[test]
    fn concurrent_create_returns_one() {
        let path = std::env::temp_dir().join(format!("gongbu-{}.db", Uuid::new_v4()));
        let r = Repository::open(&path, Redactor::default()).unwrap();
        let barrier = Arc::new(Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let r = r.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    r.create_execution(&new("a", "op")).unwrap().execution_id
                })
            })
            .collect();
        let ids: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(ids.iter().all(|x| x == &ids[0]));
        assert_eq!(r.count("executions"), 1);
        drop(r);
        std::fs::remove_file(path).unwrap()
    }
    #[test]
    fn optimistic_concurrency_rejects_stale() {
        let r = Repository::in_memory().unwrap();
        let e = r.create_execution(&new("a", "op")).unwrap();
        let u = ExecutionUpdate {
            status: "preflighting".into(),
            outcome: None,
            started_at: Some("2026-08-05T20:01:00Z".into()),
            completed_at: None,
            failure_code: None,
            failure_message_redacted: None,
            provider_outcome: None,
            artifact_outcome: None,
            settlement_outcome: None,
        };
        assert_eq!(
            r.update_execution(&e.execution_id, 0, &u, "2026-08-05T20:01:00Z")
                .unwrap()
                .version,
            1
        );
        assert!(matches!(
            r.update_execution(&e.execution_id, 0, &u, "2026-08-05T20:02:00Z"),
            Err(Error::Stale)
        ))
    }
    #[test]
    fn v1_state_model_skips_unshipped_persisting_phase() {
        assert!(status("persisting").is_err());
        assert!(allowed_transition("executing", "settling"));
        assert!(!allowed_transition("executing", "persisting"));
    }
    #[test]
    fn restart_persistence() {
        let path = std::env::temp_dir().join(format!("gongbu-restart-{}.db", Uuid::new_v4()));
        let id = {
            Repository::open(&path, Redactor::default())
                .unwrap()
                .create_execution(&new("a", "op"))
                .unwrap()
                .execution_id
        };
        assert_eq!(
            Repository::open(&path, Redactor::default())
                .unwrap()
                .get_execution(&id)
                .unwrap()
                .operation_key,
            "op"
        );
        std::fs::remove_file(path).unwrap()
    }

    #[test]
    fn artifact_repository_rejects_traversing_ids_and_keys() {
        let repository = Repository::in_memory().unwrap();
        let execution = repository
            .create_execution(&new("account", "traversal"))
            .unwrap();
        let params = CreateArtifactParams {
            artifact_id: "../escape".into(),
            execution_id: execution.execution_id.clone(),
            provider_attempt_id: None,
            kind: "image".into(),
            storage_backend: "local_fs".into(),
            media_type: "image/png".into(),
            storage_key: format!("executions/{}/../escape.png", execution.execution_id),
            size_bytes: 1,
            sha256: "a".repeat(64),
            metadata: json!({}),
            metadata_schema_version: 1,
            created_at: "2026-08-05T20:02:00Z".into(),
        };
        assert!(matches!(
            repository.create_artifact(&params),
            Err(Error::Invalid("artifact"))
        ));
        assert_eq!(repository.count("artifacts"), 0);
    }
    #[test]
    fn cascade_without_receipt_and_restrict_with_receipt() {
        let r = Repository::in_memory().unwrap();
        let e = r.create_execution(&new("a", "cascade")).unwrap();
        let a = attempt(&r, &e);
        r.create_artifact(&CreateArtifactParams {
            artifact_id: "cascade-artifact".into(),
            execution_id: e.execution_id.clone(),
            provider_attempt_id: Some(a),
            kind: "image".into(),
            storage_backend: "local_fs".into(),
            media_type: "image/png".into(),
            storage_key: format!("executions/{}/cascade-artifact.png", e.execution_id),
            size_bytes: 1,
            sha256: "a".repeat(64),
            metadata: json!({}),
            metadata_schema_version: 1,
            created_at: "2026-08-05T20:02:00Z".into(),
        })
        .unwrap();
        r.delete(&e.execution_id).unwrap();
        assert_eq!(r.count("provider_attempts"), 0);
        assert_eq!(r.count("artifacts"), 0);
        let e = r.create_execution(&new("a", "restrict")).unwrap();
        let a = attempt(&r, &e);
        complete_success(&r, &a);
        r.create_receipt(&CreateReceiptParams {
            receipt_id: "r".into(),
            execution_id: e.execution_id.clone(),
            provider_attempt_id: a,
            settlement_minor: 100,
            currency: "USD".into(),
            pricing_catalog_version: "prices-v2".into(),
            actual_vendor_cost: ActualVendorCost::new(100, 2, "USD").unwrap(),
            created_at: "2026-08-05T20:03:00Z".into(),
            settled_at: None,
            hubu_settlement_id: None,
        })
        .unwrap();
        assert!(r.delete(&e.execution_id).is_err())
    }
    #[test]
    fn multiple_children_and_attempt_completion() {
        let r = Repository::in_memory().unwrap();
        let e = r.create_execution(&new("a", "op")).unwrap();
        for n in 0..2 {
            let a = attempt(&r, &e);
            r.complete_provider_attempt(
                &a,
                &AttemptResult {
                    outcome: "succeeded".into(),
                    completed_at: "2026-08-05T20:02:00Z".into(),
                    usage: json!({"images":1}),
                    usage_schema_version: 1,
                    actual_vendor_cost: Some(ActualVendorCost::new(100, 2, "USD").unwrap()),
                    failure_code: None,
                    failure_message_redacted: None,
                    provider_request_id: Some(format!("provider-request-{n}")),
                    provider_operation_id: None,
                },
            )
            .unwrap();
            r.create_artifact(&CreateArtifactParams {
                artifact_id: format!("artifact-{n}"),
                execution_id: e.execution_id.clone(),
                provider_attempt_id: Some(a),
                kind: "image".into(),
                storage_backend: "local_fs".into(),
                media_type: "image/png".into(),
                storage_key: format!("executions/{}/artifact-{n}.png", e.execution_id),
                size_bytes: 1,
                sha256: format!("{n}").repeat(64),
                metadata: json!({}),
                metadata_schema_version: 1,
                created_at: "2026-08-05T20:02:00Z".into(),
            })
            .unwrap();
        }
        assert_eq!(r.count("provider_attempts"), 2);
        assert_eq!(r.count("artifacts"), 2)
    }
    #[test]
    fn create_operations_return_persisted_core_models() {
        let r = Repository::in_memory().unwrap();
        let execution = r.create_execution(&new("a", "models")).unwrap();
        let attempt = r
            .create_provider_attempt(&CreateProviderAttemptParams {
                execution_id: execution.execution_id.clone(),
                provider: "mock".into(),
                provider_request_id: Some("request-1".into()),
                provider_operation_id: None,
                started_at: "2026-08-05T20:01:00Z".into(),
            })
            .unwrap();
        assert_eq!(attempt.execution_id, execution.execution_id);
        assert_eq!(attempt.outcome, "started");
        let artifact = r
            .create_artifact(&CreateArtifactParams {
                artifact_id: "artifact-model".into(),
                execution_id: execution.execution_id.clone(),
                provider_attempt_id: Some(attempt.provider_attempt_id.clone()),
                kind: "image".into(),
                storage_backend: "local_fs".into(),
                media_type: "image/png".into(),
                storage_key: format!("executions/{}/artifact-model.png", execution.execution_id),
                size_bytes: 1,
                sha256: "a".repeat(64),
                metadata: json!({}),
                metadata_schema_version: 1,
                created_at: "2026-08-05T20:02:00Z".into(),
            })
            .unwrap();
        assert_eq!(
            artifact.provider_attempt_id,
            Some(attempt.provider_attempt_id.clone())
        );
        complete_success(&r, &attempt.provider_attempt_id);
        let receipt = r
            .create_receipt(&CreateReceiptParams {
                receipt_id: "receipt-model".into(),
                execution_id: execution.execution_id.clone(),
                provider_attempt_id: attempt.provider_attempt_id,
                settlement_minor: 100,
                currency: "USD".into(),
                pricing_catalog_version: "prices-v2".into(),
                actual_vendor_cost: ActualVendorCost::new(100, 2, "USD").unwrap(),
                created_at: "2026-08-05T20:03:00Z".into(),
                settled_at: None,
                hubu_settlement_id: None,
            })
            .unwrap();
        assert_eq!(receipt.receipt_id, "receipt-model");
        assert_eq!(receipt.settlement_minor, 100);
        assert_eq!(receipt.provider_request_id, "provider-request");
        assert_eq!(receipt.price_model_snapshot, execution.pricing_snapshot);
    }
    #[test]
    fn receipt_requires_a_successfully_completed_attempt() {
        let r = Repository::in_memory().unwrap();
        let execution = r.create_execution(&new("a", "billable-only")).unwrap();
        let started = attempt(&r, &execution);
        let receipt_for = |attempt_id: String| CreateReceiptParams {
            receipt_id: format!("receipt-{attempt_id}"),
            execution_id: execution.execution_id.clone(),
            provider_attempt_id: attempt_id,
            settlement_minor: 100,
            currency: "USD".into(),
            pricing_catalog_version: "prices-v2".into(),
            actual_vendor_cost: ActualVendorCost::new(100, 2, "USD").unwrap(),
            created_at: "2026-08-05T20:03:00Z".into(),
            settled_at: None,
            hubu_settlement_id: None,
        };
        assert!(matches!(
            r.create_receipt(&receipt_for(started)),
            Err(Error::Invalid("receipt requires a succeeded attempt"))
        ));
        let failed = attempt(&r, &execution);
        r.complete_provider_attempt(
            &failed,
            &AttemptResult {
                outcome: "failed".into(),
                completed_at: "2026-08-05T20:02:00Z".into(),
                usage: json!({}),
                usage_schema_version: 1,
                actual_vendor_cost: None,
                failure_code: Some("provider_error".into()),
                failure_message_redacted: Some("provider request failed".into()),
                provider_request_id: None,
                provider_operation_id: None,
            },
        )
        .unwrap();
        assert!(matches!(
            r.create_receipt(&receipt_for(failed)),
            Err(Error::Invalid("receipt requires a succeeded attempt"))
        ));
    }
    #[test]
    fn secrets_and_authorization_are_guarded() {
        assert!(HubuTokenReference::new("eyJhbGciOi.x.y").is_err());
        assert!(HubuTokenReference::new("  Bearer opaque-token").is_err());
        let r = Repository::in_memory().unwrap();
        let mut bad = new("a", "bad");
        bad.normalized_input = json!({"api_key":"oops"});
        assert!(r.create_execution(&bad).is_err());
        let e = r.create_execution(&new("a", "ok")).unwrap();
        let a = attempt(&r, &e);
        complete_success(&r, &a);
        let mut receipt = CreateReceiptParams {
            receipt_id: "r".into(),
            execution_id: e.execution_id,
            provider_attempt_id: a,
            settlement_minor: 101,
            currency: "USD".into(),
            pricing_catalog_version: "prices-v2".into(),
            actual_vendor_cost: ActualVendorCost::new(100, 2, "USD").unwrap(),
            created_at: "2026-08-05T20:02:00Z".into(),
            settled_at: None,
            hubu_settlement_id: None,
        };
        assert!(matches!(
            r.create_receipt(&receipt),
            Err(Error::OverAuthorization)
        ));
        receipt.settlement_minor = 100;
        receipt.pricing_catalog_version = "stale-version".into();
        assert!(matches!(
            r.create_receipt(&receipt),
            Err(Error::Invalid("pricing catalog version"))
        ));
        receipt.settlement_minor = 501;
        receipt.pricing_catalog_version = "prices-v2".into();
        assert!(matches!(
            r.create_receipt(&receipt),
            Err(Error::OverAuthorization)
        ));
        let other = r.create_execution(&new("b", "other")).unwrap();
        let cross_artifact = CreateArtifactParams {
            artifact_id: "cross-artifact".into(),
            execution_id: other.execution_id.clone(),
            provider_attempt_id: Some(receipt.provider_attempt_id.clone()),
            kind: "image".into(),
            storage_backend: "local_fs".into(),
            media_type: "image/png".into(),
            storage_key: format!("executions/{}/cross-artifact.png", other.execution_id),
            size_bytes: 1,
            sha256: "a".repeat(64),
            metadata: json!({}),
            metadata_schema_version: 1,
            created_at: "2026-08-05T20:03:00Z".into(),
        };
        assert!(matches!(
            r.create_artifact(&cross_artifact),
            Err(Error::Invalid("artifact attempt relationship"))
        ));
        let mismatched = CreateReceiptParams {
            receipt_id: "cross-aggregate".into(),
            execution_id: other.execution_id.clone(),
            provider_attempt_id: receipt.provider_attempt_id,
            settlement_minor: 1,
            currency: "USD".into(),
            pricing_catalog_version: "v1".into(),
            actual_vendor_cost: ActualVendorCost::new(1, 2, "USD").unwrap(),
            created_at: "2026-08-05T20:03:00Z".into(),
            settled_at: None,
            hubu_settlement_id: None,
        };
        assert!(matches!(
            r.create_receipt(&mismatched),
            Err(Error::Invalid("receipt attempt relationship"))
        ));
    }

    #[test]
    fn provider_operation_checkpoint_is_durable_idempotent_and_conflict_safe() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("provider-operation.sqlite3");
        let repository = Repository::open(&path, Redactor::default()).unwrap();
        let execution = repository
            .create_execution(&new("account", "async-checkpoint"))
            .unwrap();
        let attempt_id = attempt(&repository, &execution);
        let operation = AsyncProviderOperation {
            provider_request_id: Some("request-170".into()),
            provider_operation_id: "operation-170".into(),
            polling_host: "api.bfl.ai".into(),
            polling_recovery: Some(PollingRecoveryContext {
                schema_version: 1,
                policy_version: "bfl-polling-origin-v2".into(),
                scheme: Some("https".into()),
                normalized_host: Some("api.bfl.ai".into()),
                explicit_port: None,
                endpoint_shape: "v1/get_result".into(),
                query_keys: vec!["id".into()],
                url_fingerprint: format!("sha256:{}", "a".repeat(64)),
                validation_reason: None,
            }),
            deadline_unix_ms: 1_799_999_999_000,
        };

        let stored = repository
            .record_provider_operation(&attempt_id, &operation, "2026-08-28T18:00:01Z")
            .unwrap();
        assert_eq!(
            repository.provider_operation(&stored).unwrap(),
            Some(operation.clone())
        );
        assert_eq!(stored.provider_request_id, operation.provider_request_id);
        assert_eq!(
            stored.provider_operation_id.as_deref(),
            Some("operation-170")
        );
        assert_eq!(stored.provider_polling_host.as_deref(), Some("api.bfl.ai"));
        assert_eq!(
            stored.provider_deadline_unix_ms,
            Some(operation.deadline_unix_ms)
        );
        assert_eq!(
            stored.operation_checkpointed_at.as_deref(),
            Some("2026-08-28T18:00:01Z")
        );
        assert_eq!(repository.count("provider_attempts"), 1);
        drop(repository);

        let restarted = Repository::open(&path, Redactor::default()).unwrap();
        let reopened = restarted.get_provider_attempt(&attempt_id).unwrap();
        assert_eq!(
            restarted.provider_operation(&reopened).unwrap(),
            Some(operation.clone())
        );
        assert_eq!(
            restarted
                .record_provider_operation(&attempt_id, &operation, "2026-08-28T18:00:01Z",)
                .unwrap(),
            reopened
        );

        let conflicting = AsyncProviderOperation {
            provider_operation_id: "operation-171".into(),
            ..operation.clone()
        };
        assert!(matches!(
            restarted.record_provider_operation(&attempt_id, &conflicting, "2026-08-28T18:00:01Z"),
            Err(Error::Stale)
        ));
        assert_eq!(
            restarted
                .provider_operation(&restarted.get_provider_attempt(&attempt_id).unwrap())
                .unwrap(),
            Some(operation)
        );
        assert_eq!(restarted.count("provider_attempts"), 1);
    }

    #[test]
    fn successful_checkpoint_preserves_downstream_reconciliation_progress() {
        let repository = Repository::in_memory().unwrap();
        let execution = repository
            .create_execution(&new("account", "downstream-reconciliation"))
            .unwrap();
        let attempt_id = attempt(&repository, &execution);
        let interrupted = repository
            .record_reconciliation(
                &execution,
                "executing",
                Some("provider_submission_interrupted"),
                "2026-09-03T21:00:00Z",
            )
            .unwrap();
        assert_eq!(interrupted.last_confirmed_step, "executing");
        assert_eq!(
            interrupted.evidence["recovery_guidance"]["provider_outcome_ambiguous"],
            true
        );
        assert_eq!(
            interrupted.evidence["recovery_guidance"]["do_not_resubmit"],
            true
        );
        repository
            .record_provider_operation(
                &attempt_id,
                &AsyncProviderOperation {
                    provider_request_id: Some("request-200".into()),
                    provider_operation_id: "operation-200".into(),
                    polling_host: "api.us.bfl.ai".into(),
                    polling_recovery: Some(PollingRecoveryContext {
                        schema_version: 1,
                        policy_version: "bfl-polling-origin-v2".into(),
                        scheme: Some("https".into()),
                        normalized_host: Some("api.us.bfl.ai".into()),
                        explicit_port: None,
                        endpoint_shape: "v1/get_result".into(),
                        query_keys: vec!["id".into()],
                        url_fingerprint: format!("sha256:{}", "a".repeat(64)),
                        validation_reason: None,
                    }),
                    deadline_unix_ms: 1_799_999_999_000,
                },
                "2026-09-03T21:00:01Z",
            )
            .unwrap();
        complete_success(&repository, &attempt_id);

        for step in ["executing", "settling"] {
            let record = repository
                .record_reconciliation(
                    &execution,
                    step,
                    Some("downstream_phase_failed"),
                    "2026-09-03T21:00:02Z",
                )
                .unwrap();
            assert_eq!(record.last_confirmed_step, step);
            assert_eq!(record.evidence["last_confirmed_step"], step);
            assert_eq!(record.evidence["provider_outcome"], "succeeded");
            assert!(record.evidence.get("recovery_guidance").is_none());
            assert_eq!(
                record.evidence["polling_recovery"]["normalized_host"],
                "api.us.bfl.ai"
            );
        }
    }

    #[test]
    fn provider_transport_counters_are_monotonic_restart_durable_and_terminal_safe() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("provider-transport.sqlite3");
        let repository = Repository::open(&path, Redactor::default()).unwrap();
        let mut params = new("account", "transport-counters");
        params.hubu_claim_id = None;
        let pending = repository.create_execution(&params).unwrap();
        let preflighting = repository
            .update_execution(
                &pending.execution_id,
                pending.version,
                &ExecutionUpdate {
                    status: "preflighting".into(),
                    outcome: None,
                    started_at: None,
                    completed_at: None,
                    failure_code: None,
                    failure_message_redacted: None,
                    provider_outcome: None,
                    artifact_outcome: None,
                    settlement_outcome: None,
                },
                "2026-08-05T20:00:01Z",
            )
            .unwrap();
        let claimed = repository
            .set_claim(
                &preflighting.execution_id,
                preflighting.version,
                "claim-transport-counters",
                "2026-08-05T20:00:02Z",
            )
            .unwrap();
        let attempt_id = repository
            .start_provider_attempt(&claimed, "2026-08-05T20:00:03Z")
            .unwrap()
            .provider_attempt_id;

        assert!(matches!(
            repository.record_provider_poll(&attempt_id),
            Err(Error::Stale)
        ));
        let pretransmission = repository.get_provider_attempt(&attempt_id).unwrap();
        assert_eq!(pretransmission.provider_poll_count, 0);
        assert_eq!(pretransmission.artifact_fetch_count, 0);
        repository
            .begin_provider_transmission(&attempt_id, "2026-08-05T20:00:04Z")
            .unwrap();

        repository.record_provider_poll(&attempt_id).unwrap();
        repository.record_provider_poll(&attempt_id).unwrap();
        repository.record_artifact_fetch(&attempt_id).unwrap();
        let before_restart = repository.get_provider_attempt(&attempt_id).unwrap();
        assert_eq!(before_restart.provider_poll_count, 2);
        assert_eq!(before_restart.artifact_fetch_count, 1);
        drop(repository);

        let restarted = Repository::open(&path, Redactor::default()).unwrap();
        let recovered = restarted.get_provider_attempt(&attempt_id).unwrap();
        assert_eq!(recovered.provider_poll_count, 2);
        assert_eq!(recovered.artifact_fetch_count, 1);
        restarted.record_provider_poll(&attempt_id).unwrap();
        let cumulative = restarted.get_provider_attempt(&attempt_id).unwrap();
        assert_eq!(cumulative.provider_poll_count, 3);
        assert_eq!(cumulative.artifact_fetch_count, 1);

        complete_success(&restarted, &attempt_id);
        assert!(matches!(
            restarted.record_artifact_fetch(&attempt_id),
            Err(Error::Stale)
        ));
        let terminal = restarted.get_provider_attempt(&attempt_id).unwrap();
        assert_eq!(terminal.provider_poll_count, 3);
        assert_eq!(terminal.artifact_fetch_count, 1);
    }

    #[test]
    fn provider_operation_checkpoint_rejects_secrets_and_unsafe_evidence_atomically() {
        const CANARY: &str = "gongbu-checkpoint-secret-170";
        let repository =
            Repository::in_memory_with_redactor(Redactor::new([CANARY.as_bytes()])).unwrap();
        let execution = repository
            .create_execution(&new("account", "safe-checkpoint"))
            .unwrap();
        let attempt_id = attempt(&repository, &execution);
        let secret_bearing = AsyncProviderOperation {
            provider_request_id: Some(CANARY.into()),
            provider_operation_id: "operation-safe".into(),
            polling_host: "api.bfl.ai".into(),
            polling_recovery: None,
            deadline_unix_ms: 1_799_999_999_000,
        };

        assert!(matches!(
            repository.record_provider_operation(
                &attempt_id,
                &secret_bearing,
                "2026-08-28T18:00:01Z"
            ),
            Err(Error::Invalid("secret-bearing persistence value"))
        ));
        let unsafe_url = AsyncProviderOperation {
            provider_request_id: Some("request-safe".into()),
            provider_operation_id: "https://example.invalid/result?token=signed".into(),
            polling_host: "api.bfl.ai".into(),
            polling_recovery: None,
            deadline_unix_ms: 1_799_999_999_000,
        };
        assert!(matches!(
            repository.record_provider_operation(&attempt_id, &unsafe_url, "2026-08-28T18:00:01Z"),
            Err(Error::Invalid("provider operation checkpoint"))
        ));

        let unchanged = repository.get_provider_attempt(&attempt_id).unwrap();
        assert_eq!(repository.provider_operation(&unchanged).unwrap(), None);
        assert_eq!(unchanged.provider_request_id, None);
        assert_eq!(unchanged.provider_operation_id, None);
        assert_eq!(unchanged.provider_polling_host, None);
        assert_eq!(unchanged.provider_deadline_unix_ms, None);
        assert_eq!(unchanged.operation_checkpointed_at, None);
        assert_eq!(repository.count("provider_attempts"), 1);
    }

    #[test]
    fn sandbox_fixture_metadata_persists_while_service_credentials_remain_guarded() {
        const CALLER_SECRET: &str = "sandbox-caller-capability-181";
        const HUBU_SECRET: &str = "sandbox-hubu-capability-181";
        let repository = Repository::in_memory_with_redactor(Redactor::new([
            CALLER_SECRET.as_bytes(),
            HUBU_SECRET.as_bytes(),
        ]))
        .unwrap();
        let mut request = new("sandbox-account", "sandbox-operation");
        request.provider = "sandbox".into();
        request.adapter = "fixture".into();
        request.model = "deterministic-image-v1".into();
        request.provider_config_version = "hubu-sandbox-fixture-v1".into();
        request.normalized_input = json!({"prompt":"prompt mentions sandbox-fixture"});
        request.pricing_snapshot["provider"] = json!("sandbox");
        request.pricing_snapshot["model"] = json!("deterministic-image-v1");

        let execution = repository.create_execution(&request).unwrap();
        assert_eq!(execution.provider_config_version, "hubu-sandbox-fixture-v1");
        assert_eq!(
            execution.normalized_input,
            json!({"prompt":"prompt mentions sandbox-fixture"})
        );

        request.operation_key = CALLER_SECRET.into();
        assert!(matches!(
            repository.create_execution(&request),
            Err(Error::Invalid("secret-bearing persistence value"))
        ));
    }

    #[test]
    fn canary_secret_is_redacted_from_failures_and_rejected_from_records() {
        const CANARY: &str = "gongbu-canary-provider-secret-7c91";
        let r = Repository::in_memory_with_redactor(Redactor::new([CANARY.as_bytes()])).unwrap();
        let mut leaked = new("a", "leaked-input");
        leaked.normalized_input = json!({"prompt": CANARY});
        assert!(matches!(
            r.create_execution(&leaked),
            Err(Error::Invalid("secret-bearing persistence value"))
        ));
        let mut leaked_claim = new("a", "leaked-claim");
        leaked_claim.hubu_claim_id = Some(CANARY.into());
        assert!(matches!(
            r.create_execution(&leaked_claim),
            Err(Error::Invalid("secret-bearing persistence value"))
        ));
        let mut leaked_reference = new("a", "leaked-reference");
        leaked_reference.hubu_authorization_id = CANARY.into();
        leaked_reference.hubu_token_reference = HubuTokenReference::new(CANARY).unwrap();
        assert!(matches!(
            r.create_execution(&leaked_reference),
            Err(Error::Invalid("secret-bearing persistence value"))
        ));

        let escaped_canary = "gongbu-canary-\"slash\\newline\nsecret";
        let escaped_repo =
            Repository::in_memory_with_redactor(Redactor::new([escaped_canary.as_bytes()]))
                .unwrap();
        let execution = escaped_repo.create_execution(&new("a", "escaped")).unwrap();
        let result = escaped_repo.create_provider_attempt(&CreateProviderAttemptParams {
            execution_id: execution.execution_id,
            provider: "vendor".into(),
            provider_request_id: Some(escaped_canary.into()),
            provider_operation_id: None,
            started_at: "now".into(),
        });
        assert!(matches!(
            result,
            Err(Error::Invalid("secret-bearing persistence value"))
        ));
        let mut escaped_json = new("a", "escaped-json");
        escaped_json.normalized_input = json!({"prompt": escaped_canary});
        assert!(matches!(
            escaped_repo.create_execution(&escaped_json),
            Err(Error::Invalid("secret-bearing persistence value"))
        ));

        let e = r.create_execution(&new("a", "redacted-failure")).unwrap();
        let a = attempt(&r, &e);
        let leaked_request_id = AttemptResult {
            outcome: "succeeded".into(),
            completed_at: "now".into(),
            usage: json!({}),
            usage_schema_version: 1,
            actual_vendor_cost: None,
            failure_code: None,
            failure_message_redacted: None,
            provider_request_id: Some(CANARY.into()),
            provider_operation_id: None,
        };
        assert!(matches!(
            r.complete_provider_attempt(&a, &leaked_request_id),
            Err(Error::Invalid("secret-bearing persistence value"))
        ));
        r.complete_provider_attempt(
            &a,
            &AttemptResult {
                outcome: "failed".into(),
                completed_at: "now".into(),
                usage: json!({}),
                usage_schema_version: 1,
                actual_vendor_cost: None,
                failure_code: Some("provider_error".into()),
                failure_message_redacted: Some(format!("nested SDK error: api_key={CANARY}")),
                provider_request_id: None,
                provider_operation_id: None,
            },
        )
        .unwrap();
        let stored: String = r.0.lock().unwrap().query_row(
            "SELECT failure_message_redacted FROM provider_attempts WHERE provider_attempt_id=?1",
            [&a], |row| row.get(0)).unwrap();
        assert!(!stored.contains(CANARY));
        assert!(stored.contains("[REDACTED]"));

        let numeric_secret = "777777";
        let numeric_repo =
            Repository::in_memory_with_redactor(Redactor::new([numeric_secret.as_bytes()]))
                .unwrap();
        let mut numeric = new("a", "numeric");
        numeric.authorized_minor = 777777;
        assert!(matches!(
            numeric_repo.create_execution(&numeric),
            Err(Error::Invalid("secret-bearing persistence value"))
        ));
        let fixed_repo =
            Repository::in_memory_with_redactor(Redactor::new([b"pending".as_slice()])).unwrap();
        assert!(matches!(
            fixed_repo.create_execution(&new("a", "fixed-status")),
            Err(Error::Invalid("secret-bearing persistence value"))
        ));
    }
}
