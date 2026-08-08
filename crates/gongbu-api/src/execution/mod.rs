//! SQLite persistence for the execution aggregate.
//! Schema units and formats are documented in the migration.
use crate::provider_contract::{PricingSnapshot, PRICING_SNAPSHOT_SCHEMA_VERSION};
use crate::redaction::Redactor;
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
    pub created_at: String,
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
    pub outcome: String,
    pub usage: Option<Value>,
    pub usage_schema_version: Option<i64>,
    pub provider_amount_minor: Option<i64>,
    pub provider_currency: Option<String>,
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
    pub provider_amount_minor: Option<i64>,
    pub provider_currency: Option<String>,
    pub failure_code: Option<String>,
    pub failure_message_redacted: Option<String>,
    pub provider_request_id: Option<String>,
    pub provider_operation_id: Option<String>,
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
        migrate_artifact_storage_columns(&c)?;
        migrate_resolved_target_columns(&c)?;
        Ok(Self(Arc::new(Mutex::new(c)), redactor))
    }
    pub fn create_execution(&self, n: &CreateExecutionParams) -> Result<Execution> {
        validate_execution(n)?;
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
        self.reject_registered_json([&n.normalized_input, &n.pricing_snapshot])?;
        self.reject_registered_secrets([normalized_input.as_str(), pricing_snapshot.as_str()])?;
        let mut c = self.0.lock().unwrap();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("INSERT OR IGNORE INTO executions(execution_id,account_id,operation_key,hubu_authorization_id,hubu_claim_id,hubu_token_reference,authorized_minor,authorization_currency,normalized_input_json,input_hash,input_schema_version,target,config_version,workload_type,provider,adapter,model,provider_config_version,provider_config_digest,pricing_snapshot_json,pricing_schema_version,status,created_at,updated_at,version) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,'pending',?22,?22,0)",params![id,n.account_id,n.operation_key,n.hubu_authorization_id,n.hubu_claim_id,n.hubu_token_reference.0,n.authorized_minor,n.authorization_currency,j(&n.normalized_input),n.input_hash,n.input_schema_version,n.target,n.config_version,n.workload_type,n.provider,n.adapter,n.model,n.provider_config_version,n.provider_config_digest,j(&n.pricing_snapshot),n.pricing_schema_version,n.created_at])?;
        let e = query_key(&tx, &n.account_id, &n.operation_key)?;
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
            outcome: "started".into(),
            usage: None,
            usage_schema_version: None,
            provider_amount_minor: None,
            provider_currency: None,
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
    pub fn complete_provider_attempt(&self, id: &str, r: &AttemptResult) -> Result<()> {
        safe_usage_json(&r.usage)?;
        if r.usage_schema_version < 1
            || r.provider_amount_minor.is_some() != r.provider_currency.is_some()
            || r.provider_amount_minor.is_some_and(|v| v < 0)
        {
            return Err(Error::Invalid("attempt result"));
        }
        let failure = r
            .failure_message_redacted
            .as_deref()
            .map(|value| self.1.redact(value));
        let usage = j(&r.usage);
        let numeric = [Some(r.usage_schema_version), r.provider_amount_minor];
        self.reject_registered_numbers(numeric.into_iter().flatten())?;
        self.reject_registered_json([&r.usage])?;
        self.reject_registered_secrets([
            r.outcome.as_str(),
            r.completed_at.as_str(),
            usage.as_str(),
            r.provider_currency.as_deref().unwrap_or(""),
            r.failure_code.as_deref().unwrap_or(""),
            failure.as_deref().unwrap_or(""),
            r.provider_request_id.as_deref().unwrap_or(""),
            r.provider_operation_id.as_deref().unwrap_or(""),
            id,
        ])?;
        let n=self.0.lock().unwrap().execute("UPDATE provider_attempts SET outcome=?1,completed_at=?2,usage_json=?3,usage_schema_version=?4,provider_amount_minor=?5,provider_currency=?6,failure_code=?7,failure_message_redacted=?8,provider_request_id=?9,provider_operation_id=?10 WHERE provider_attempt_id=?11 AND completed_at IS NULL AND transmission_started_at IS NOT NULL",params![r.outcome,r.completed_at,j(&r.usage),r.usage_schema_version,r.provider_amount_minor,r.provider_currency,r.failure_code,failure,r.provider_request_id,r.provider_operation_id,id])?;
        if n == 1 {
            Ok(())
        } else {
            Err(Error::NotFound)
        }
    }
    pub fn get_provider_attempt_for_execution(
        &self,
        execution_id: &str,
    ) -> Result<ProviderAttempt> {
        self.0.lock().unwrap().query_row("SELECT provider_attempt_id,execution_id,provider,provider_request_id,provider_operation_id,outcome,usage_json,usage_schema_version,provider_amount_minor,provider_currency,failure_code,failure_message_redacted,started_at,transmission_started_at,completed_at FROM provider_attempts WHERE execution_id=?1", [execution_id], |r| {
            let usage: Option<String> = r.get(6)?;
            Ok(ProviderAttempt { provider_attempt_id:r.get(0)?, execution_id:r.get(1)?, provider:r.get(2)?, provider_request_id:r.get(3)?, provider_operation_id:r.get(4)?, outcome:r.get(5)?, usage:usage.map(|v| serde_json::from_str(&v).unwrap()), usage_schema_version:r.get(7)?, provider_amount_minor:r.get(8)?, provider_currency:r.get(9)?, failure_code:r.get(10)?, failure_message_redacted:r.get(11)?, started_at:r.get(12)?, transmission_started_at:r.get(13)?, completed_at:r.get(14)? })
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
        self.reject_registered_secrets([
            n.receipt_id.as_str(),
            n.execution_id.as_str(),
            n.provider_attempt_id.as_str(),
            n.currency.as_str(),
            n.pricing_catalog_version.as_str(),
            n.created_at.as_str(),
            n.settled_at.as_deref().unwrap_or(""),
            n.hubu_settlement_id.as_deref().unwrap_or(""),
        ])?;
        self.reject_registered_numbers([n.settlement_minor])?;
        if n.settlement_minor < 0 {
            return Err(Error::Invalid("settlement"));
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
        let (attempt_execution, attempt_outcome, attempt_completed_at, provider_request_id, provider_operation_id): (
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = c
            .query_row(
                "SELECT execution_id,outcome,completed_at,provider_request_id,provider_operation_id FROM provider_attempts WHERE provider_attempt_id=?1",
                [&n.provider_attempt_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?
            .ok_or(Error::NotFound)?;
        if attempt_execution != n.execution_id {
            return Err(Error::Invalid("receipt attempt relationship"));
        }
        if attempt_outcome != "succeeded"
            || attempt_completed_at.is_none()
            || (provider_request_id.is_none() && provider_operation_id.is_none())
        {
            return Err(Error::Invalid("receipt requires a succeeded attempt"));
        }
        let snapshot: PricingSnapshot =
            serde_json::from_str(&auth.2).map_err(|_| Error::Invalid("pricing snapshot"))?;
        snapshot
            .validate_integrity()
            .map_err(|_| Error::Invalid("pricing snapshot"))?;
        if n.pricing_catalog_version != snapshot.catalog_version {
            return Err(Error::Invalid("pricing catalog version"));
        }
        if n.settlement_minor > auth.0
            || n.settlement_minor > snapshot.estimated_amount_minor
            || !n.currency.eq_ignore_ascii_case(&auth.1)
            || !n.currency.eq_ignore_ascii_case(&snapshot.currency)
        {
            return Err(Error::OverAuthorization);
        }
        c.execute(
            "INSERT INTO receipts(receipt_id,execution_id,provider_attempt_id,settlement_minor,currency,pricing_catalog_version,created_at,settled_at,hubu_settlement_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                n.receipt_id,
                n.execution_id,
                n.provider_attempt_id,
                n.settlement_minor,
                n.currency,
                n.pricing_catalog_version,
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
            created_at: n.created_at.clone(),
            transmission_started_at: None,
            settled_at: n.settled_at.clone(),
            hubu_settlement_id: n.hubu_settlement_id.clone(),
        })
    }
    pub fn get_receipt_for_execution(&self, execution_id: &str) -> Result<Receipt> {
        self.0.lock().unwrap().query_row("SELECT receipt_id,execution_id,provider_attempt_id,settlement_minor,currency,pricing_catalog_version,created_at,transmission_started_at,settled_at,hubu_settlement_id FROM receipts WHERE execution_id=?1",[execution_id],|r| Ok(Receipt { receipt_id:r.get(0)?,execution_id:r.get(1)?,provider_attempt_id:r.get(2)?,settlement_minor:r.get(3)?,currency:r.get(4)?,pricing_catalog_version:r.get(5)?,created_at:r.get(6)?,transmission_started_at:r.get(7)?,settled_at:r.get(8)?,hubu_settlement_id:r.get(9)? })).optional()?.ok_or(Error::NotFound)
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
        let evidence = serde_json::json!({
            "execution_id": execution.execution_id,
            "provider_attempt_id": attempt.as_ref().map(|a| &a.provider_attempt_id),
            "provider_request_id": attempt.as_ref().and_then(|a| a.provider_request_id.as_ref()),
            "provider_operation_id": attempt.as_ref().and_then(|a| a.provider_operation_id.as_ref()),
            "timestamps": {"created_at": execution.created_at, "updated_at": at, "started_at": execution.started_at, "attempt_started_at": attempt.as_ref().map(|a| &a.started_at), "transmission_started_at": attempt.as_ref().and_then(|a| a.transmission_started_at.as_ref()), "attempt_completed_at": attempt.as_ref().and_then(|a| a.completed_at.as_ref())},
            "last_confirmed_step": last_confirmed_step,
            "redacted_error": redacted_error.map(|v| self.1.redact(v)),
            "pricing_snapshot": execution.pricing_snapshot,
            "authorization": {"account_id": execution.account_id, "operation_key": execution.operation_key, "authorization_id": execution.hubu_authorization_id, "claim_id": execution.hubu_claim_id, "authorized_minor": execution.authorized_minor, "currency": execution.authorization_currency},
            "provider_outcome": attempt.as_ref().map(|a| &a.outcome),
            "usage": attempt.as_ref().and_then(|a| a.usage.as_ref()),
            "receipt": receipt.as_ref().map(|r| serde_json::json!({"receipt_id":r.receipt_id,"settlement_minor":r.settlement_minor,"currency":r.currency,"transmission_started_at":r.transmission_started_at,"settled_at":r.settled_at,"hubu_settlement_id":r.hubu_settlement_id})),
            "artifact_count": artifacts
        });
        self.reject_registered_json([&evidence])?;
        let c = self.0.lock().unwrap();
        c.execute("INSERT INTO reconciliation_records(execution_id,evidence_json,evidence_schema_version,last_confirmed_step,entered_at,updated_at) VALUES(?1,?2,1,?3,?4,?4) ON CONFLICT(execution_id) DO UPDATE SET evidence_json=excluded.evidence_json,last_confirmed_step=excluded.last_confirmed_step,updated_at=excluded.updated_at", params![execution.execution_id,j(&evidence),last_confirmed_step,at])?;
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
const EXECUTION_SELECT: &str = "SELECT execution_id,account_id,operation_key,hubu_authorization_id,hubu_claim_id,hubu_token_reference,authorized_minor,authorization_currency,normalized_input_json,input_hash,input_schema_version,target,config_version,workload_type,provider,adapter,model,provider_config_version,provider_config_digest,pricing_snapshot_json,pricing_schema_version,status,outcome,provider_outcome,artifact_outcome,settlement_outcome,failure_code,failure_message_redacted,created_at,updated_at,started_at,completed_at,release_transmission_started_at,version FROM executions";
fn map(r: &rusqlite::Row) -> rusqlite::Result<Execution> {
    let i: String = r.get(8)?;
    let p: String = r.get(19)?;
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
        || n.authorized_minor < 0
        || n.input_schema_version < 1
        || n.pricing_schema_version < 1
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
    if !matches!(
        n.pricing_schema_version,
        1 | PRICING_SNAPSHOT_SCHEMA_VERSION
    ) {
        return Err(Error::Invalid("pricing schema version"));
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
    use std::{collections::BTreeSet, sync::Barrier, thread};
    fn new(a: &str, k: &str) -> CreateExecutionParams {
        CreateExecutionParams {
            account_id: a.into(),
            operation_key: k.into(),
            hubu_authorization_id: "auth".into(),
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
                "provider":"example","model":"image-v1",
                "catalog_version":"prices-v1","catalog_digest":format!("sha256:{}", "a".repeat(64)),
                "pricing_rule_id":"example-image","unit":"image",
                "unit_amount_minor":100,"quantity":1,
                "estimated_amount_minor":100,"currency":"USD"
            }),
            pricing_schema_version: 1,
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
    fn complete_success(repo: &Repository, attempt_id: &str) {
        repo.complete_provider_attempt(
            attempt_id,
            &AttemptResult {
                outcome: "succeeded".into(),
                completed_at: "2026-08-05T20:02:00Z".into(),
                usage: json!({"images":1}),
                usage_schema_version: 1,
                provider_amount_minor: Some(50),
                provider_currency: Some("USD".into()),
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
        changed.pricing_snapshot["unit_amount_minor"] = json!(499);
        changed.pricing_snapshot["estimated_amount_minor"] = json!(499);
        let b = r.create_execution(&changed).unwrap();
        let c = r.create_execution(&new("b", "same")).unwrap();
        assert_eq!(a.execution_id, b.execution_id);
        assert_eq!(a.normalized_input, b.normalized_input);
        assert_eq!(a.pricing_snapshot, b.pricing_snapshot);
        assert_ne!(a.execution_id, c.execution_id)
    }

    #[test]
    fn legacy_database_migrates_provider_digest_column_and_new_rows_freeze_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy.sqlite3");
        let legacy = MIGRATION.replace(
            ", provider_config_digest TEXT NOT NULL CHECK(provider_config_digest GLOB 'sha256:*' AND length(provider_config_digest)=71)",
            "",
        );
        Connection::open(&path)
            .unwrap()
            .execute_batch(&legacy)
            .unwrap();

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

        let created = repository
            .create_execution(&new("migration", "digest"))
            .unwrap();
        assert_eq!(
            created.provider_config_digest,
            format!("sha256:{}", "a".repeat(64))
        );
        drop(repository);
        let restarted = Repository::open(&path, Redactor::default()).unwrap();
        assert_eq!(
            restarted
                .get_execution(&created.execution_id)
                .unwrap()
                .provider_config_digest,
            created.provider_config_digest
        );
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
            pricing_catalog_version: "prices-v1".into(),
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
                    provider_amount_minor: Some(50),
                    provider_currency: Some("USD".into()),
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
                pricing_catalog_version: "prices-v1".into(),
                created_at: "2026-08-05T20:03:00Z".into(),
                settled_at: None,
                hubu_settlement_id: None,
            })
            .unwrap();
        assert_eq!(receipt.receipt_id, "receipt-model");
        assert_eq!(receipt.settlement_minor, 100);
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
            pricing_catalog_version: "prices-v1".into(),
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
                provider_amount_minor: None,
                provider_currency: None,
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
            pricing_catalog_version: "prices-v1".into(),
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
        receipt.pricing_catalog_version = "prices-v1".into();
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
            provider_amount_minor: None,
            provider_currency: None,
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
                provider_amount_minor: None,
                provider_currency: None,
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
