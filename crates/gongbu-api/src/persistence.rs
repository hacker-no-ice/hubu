//! SQLite persistence for the execution aggregate.
//! Schema units and formats are documented in the migration.
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::Value;
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use thiserror::Error;
use uuid::Uuid;

const MIGRATION: &str = include_str!("../migrations/0001_execution_core.sql");
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
}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubuTokenReference(String);
impl HubuTokenReference {
    pub fn new(v: impl Into<String>) -> Result<Self> {
        let v = v.into();
        let l = v.to_ascii_lowercase();
        if v.trim().is_empty()
            || v.len() > 255
            || l.starts_with("bearer ")
            || l.starts_with("eyj")
            || v.contains('.')
        {
            Err(Error::Invalid("raw Hubu token"))
        } else {
            Ok(Self(v))
        }
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
    pub pricing_snapshot: Value,
    pub pricing_schema_version: i64,
    pub status: String,
    pub outcome: Option<String>,
    pub failure_code: Option<String>,
    pub failure_message_redacted: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub version: i64,
}
#[derive(Clone, Debug)]
pub struct ExecutionUpdate {
    pub status: String,
    pub outcome: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub failure_code: Option<String>,
    pub failure_message_redacted: Option<String>,
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
}
#[derive(Clone, Debug)]
pub struct CreateArtifactParams {
    pub execution_id: String,
    pub provider_attempt_id: Option<String>,
    pub kind: String,
    pub media_type: String,
    pub storage_key: String,
    pub byte_size: i64,
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
    pub media_type: String,
    pub storage_key: String,
    pub byte_size: i64,
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
    pub settled_at: Option<String>,
    pub hubu_settlement_id: Option<String>,
}
#[derive(Clone)]
pub struct Repository(Arc<Mutex<Connection>>);
impl Repository {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }
    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }
    fn init(c: Connection) -> Result<Self> {
        c.pragma_update(None, "foreign_keys", "ON")?;
        c.pragma_update(None, "busy_timeout", 5000)?;
        c.execute_batch(MIGRATION)?;
        Ok(Self(Arc::new(Mutex::new(c))))
    }
    pub fn create_execution(&self, n: &CreateExecutionParams) -> Result<Execution> {
        validate_execution(n)?;
        let mut c = self.0.lock().unwrap();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let id = Uuid::new_v4().to_string();
        tx.execute("INSERT OR IGNORE INTO executions VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,'pending',NULL,NULL,NULL,?16,?16,NULL,NULL,0)",params![id,n.account_id,n.operation_key,n.hubu_authorization_id,n.hubu_claim_id,n.hubu_token_reference.0,n.authorized_minor,n.authorization_currency,j(&n.normalized_input),n.input_hash,n.input_schema_version,n.target,n.config_version,j(&n.pricing_snapshot),n.pricing_schema_version,n.created_at])?;
        let e = query_key(&tx, &n.account_id, &n.operation_key)?;
        tx.commit()?;
        Ok(e)
    }
    pub fn get_execution(&self, id: &str) -> Result<Execution> {
        query_id(&self.0.lock().unwrap(), id)
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
        let changed=c.execute("UPDATE executions SET status=?1,outcome=?2,started_at=COALESCE(started_at,?3),completed_at=?4,failure_code=?5,failure_message_redacted=?6,updated_at=?7,version=version+1 WHERE execution_id=?8 AND version=?9",params![u.status,u.outcome,u.started_at,u.completed_at,u.failure_code,u.failure_message_redacted,at,id,expected])?;
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
    pub fn create_provider_attempt(
        &self,
        n: &CreateProviderAttemptParams,
    ) -> Result<ProviderAttempt> {
        if n.provider.trim().is_empty() {
            return Err(Error::Invalid("provider"));
        }
        let id = Uuid::new_v4().to_string();
        self.0.lock().unwrap().execute("INSERT INTO provider_attempts(provider_attempt_id,execution_id,provider,provider_request_id,provider_operation_id,outcome,started_at)VALUES(?1,?2,?3,?4,?5,'started',?6)",params![id,n.execution_id,n.provider,n.provider_request_id,n.provider_operation_id,n.started_at])?;
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
            completed_at: None,
        })
    }
    pub fn complete_provider_attempt(&self, id: &str, r: &AttemptResult) -> Result<()> {
        safe_json(&r.usage)?;
        if r.usage_schema_version < 1
            || r.provider_amount_minor.is_some() != r.provider_currency.is_some()
            || r.provider_amount_minor.is_some_and(|v| v < 0)
        {
            return Err(Error::Invalid("attempt result"));
        }
        let n=self.0.lock().unwrap().execute("UPDATE provider_attempts SET outcome=?1,completed_at=?2,usage_json=?3,usage_schema_version=?4,provider_amount_minor=?5,provider_currency=?6,failure_code=?7,failure_message_redacted=?8 WHERE provider_attempt_id=?9 AND completed_at IS NULL",params![r.outcome,r.completed_at,j(&r.usage),r.usage_schema_version,r.provider_amount_minor,r.provider_currency,r.failure_code,r.failure_message_redacted,id])?;
        if n == 1 {
            Ok(())
        } else {
            Err(Error::NotFound)
        }
    }
    pub fn create_artifact(&self, n: &CreateArtifactParams) -> Result<Artifact> {
        safe_json(&n.metadata)?;
        if n.byte_size < 0 || n.metadata_schema_version < 1 {
            return Err(Error::Invalid("artifact"));
        }
        let c = self.0.lock().unwrap();
        if let Some(attempt_id) = &n.provider_attempt_id {
            let attempt_execution: String = c
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
        let id = Uuid::new_v4().to_string();
        c.execute(
            "INSERT INTO artifacts VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                id,
                n.execution_id,
                n.provider_attempt_id,
                n.kind,
                n.media_type,
                n.storage_key,
                n.byte_size,
                n.sha256,
                j(&n.metadata),
                n.metadata_schema_version,
                n.created_at
            ],
        )?;
        Ok(Artifact {
            artifact_id: id,
            execution_id: n.execution_id.clone(),
            provider_attempt_id: n.provider_attempt_id.clone(),
            kind: n.kind.clone(),
            media_type: n.media_type.clone(),
            storage_key: n.storage_key.clone(),
            byte_size: n.byte_size,
            sha256: n.sha256.clone(),
            metadata: n.metadata.clone(),
            metadata_schema_version: n.metadata_schema_version,
            created_at: n.created_at.clone(),
        })
    }
    pub fn create_receipt(&self, n: &CreateReceiptParams) -> Result<Receipt> {
        if n.settlement_minor < 0 {
            return Err(Error::Invalid("settlement"));
        }
        let c = self.0.lock().unwrap();
        let auth: (i64, String) = c
            .query_row(
                "SELECT authorized_minor,authorization_currency FROM executions WHERE execution_id=?1",
                [&n.execution_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or(Error::NotFound)?;
        let attempt_execution: String = c
            .query_row(
                "SELECT execution_id FROM provider_attempts WHERE provider_attempt_id=?1",
                [&n.provider_attempt_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or(Error::NotFound)?;
        if attempt_execution != n.execution_id {
            return Err(Error::Invalid("receipt attempt relationship"));
        }
        if n.settlement_minor > auth.0 || n.currency != auth.1 {
            return Err(Error::OverAuthorization);
        }
        c.execute(
            "INSERT INTO receipts VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
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
            settled_at: n.settled_at.clone(),
            hubu_settlement_id: n.hubu_settlement_id.clone(),
        })
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
const EXECUTION_SELECT: &str = "SELECT execution_id,account_id,operation_key,hubu_authorization_id,hubu_claim_id,hubu_token_reference,authorized_minor,authorization_currency,normalized_input_json,input_hash,input_schema_version,target,config_version,pricing_snapshot_json,pricing_schema_version,status,outcome,failure_code,failure_message_redacted,created_at,updated_at,started_at,completed_at,version FROM executions";
fn map(r: &rusqlite::Row) -> rusqlite::Result<Execution> {
    let i: String = r.get(8)?;
    let p: String = r.get(13)?;
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
        pricing_snapshot: serde_json::from_str(&p).unwrap(),
        pricing_schema_version: r.get(14)?,
        status: r.get(15)?,
        outcome: r.get(16)?,
        failure_code: r.get(17)?,
        failure_message_redacted: r.get(18)?,
        created_at: r.get(19)?,
        updated_at: r.get(20)?,
        started_at: r.get(21)?,
        completed_at: r.get(22)?,
        version: r.get(23)?,
    })
}
fn validate_execution(n: &CreateExecutionParams) -> Result<()> {
    if n.account_id.trim().is_empty()
        || n.operation_key.trim().is_empty()
        || n.authorized_minor < 0
        || n.input_schema_version < 1
        || n.pricing_schema_version < 1
    {
        return Err(Error::Invalid("execution"));
    }
    safe_json(&n.normalized_input)?;
    safe_json(&n.pricing_snapshot)
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
fn status(s: &str) -> Result<()> {
    if [
        "pending",
        "running",
        "succeeded",
        "failed",
        "canceled",
        "reconciliation_required",
    ]
    .contains(&s)
    {
        Ok(())
    } else {
        Err(Error::Invalid("status"))
    }
}
fn j(v: &Value) -> String {
    serde_json::to_string(v).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{sync::Barrier, thread};
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
            pricing_snapshot: json!({"minor_per_image":100}),
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
    #[test]
    fn scoped_idempotency_and_immutable_snapshots() {
        let r = Repository::in_memory().unwrap();
        let a = r.create_execution(&new("a", "same")).unwrap();
        let mut changed = new("a", "same");
        changed.normalized_input = json!({"prompt":"changed"});
        changed.pricing_snapshot = json!({"minor_per_image":999});
        let b = r.create_execution(&changed).unwrap();
        let c = r.create_execution(&new("b", "same")).unwrap();
        assert_eq!(a.execution_id, b.execution_id);
        assert_eq!(a.normalized_input, b.normalized_input);
        assert_eq!(a.pricing_snapshot, b.pricing_snapshot);
        assert_ne!(a.execution_id, c.execution_id)
    }
    #[test]
    fn concurrent_create_returns_one() {
        let path = std::env::temp_dir().join(format!("gongbu-{}.db", Uuid::new_v4()));
        let r = Repository::open(&path).unwrap();
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
            status: "running".into(),
            outcome: None,
            started_at: Some("2026-08-05T20:01:00Z".into()),
            completed_at: None,
            failure_code: None,
            failure_message_redacted: None,
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
    fn restart_persistence() {
        let path = std::env::temp_dir().join(format!("gongbu-restart-{}.db", Uuid::new_v4()));
        let id = {
            Repository::open(&path)
                .unwrap()
                .create_execution(&new("a", "op"))
                .unwrap()
                .execution_id
        };
        assert_eq!(
            Repository::open(&path)
                .unwrap()
                .get_execution(&id)
                .unwrap()
                .operation_key,
            "op"
        );
        std::fs::remove_file(path).unwrap()
    }
    #[test]
    fn cascade_without_receipt_and_restrict_with_receipt() {
        let r = Repository::in_memory().unwrap();
        let e = r.create_execution(&new("a", "cascade")).unwrap();
        let a = attempt(&r, &e);
        r.create_artifact(&CreateArtifactParams {
            execution_id: e.execution_id.clone(),
            provider_attempt_id: Some(a),
            kind: "image".into(),
            media_type: "image/png".into(),
            storage_key: "objects/cascade".into(),
            byte_size: 1,
            sha256: "x".into(),
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
                },
            )
            .unwrap();
            r.create_artifact(&CreateArtifactParams {
                execution_id: e.execution_id.clone(),
                provider_attempt_id: Some(a),
                kind: "image".into(),
                media_type: "image/png".into(),
                storage_key: format!("objects/{n}"),
                byte_size: 1,
                sha256: format!("h{n}"),
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
                execution_id: execution.execution_id.clone(),
                provider_attempt_id: Some(attempt.provider_attempt_id.clone()),
                kind: "image".into(),
                media_type: "image/png".into(),
                storage_key: "objects/model-output".into(),
                byte_size: 1,
                sha256: "hash".into(),
                metadata: json!({}),
                metadata_schema_version: 1,
                created_at: "2026-08-05T20:02:00Z".into(),
            })
            .unwrap();
        assert_eq!(
            artifact.provider_attempt_id,
            Some(attempt.provider_attempt_id.clone())
        );
        let receipt = r
            .create_receipt(&CreateReceiptParams {
                receipt_id: "receipt-model".into(),
                execution_id: execution.execution_id,
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
    fn secrets_and_authorization_are_guarded() {
        assert!(HubuTokenReference::new("eyJhbGciOi.x.y").is_err());
        let r = Repository::in_memory().unwrap();
        let mut bad = new("a", "bad");
        bad.normalized_input = json!({"api_key":"oops"});
        assert!(r.create_execution(&bad).is_err());
        let e = r.create_execution(&new("a", "ok")).unwrap();
        let a = attempt(&r, &e);
        let receipt = CreateReceiptParams {
            receipt_id: "r".into(),
            execution_id: e.execution_id,
            provider_attempt_id: a,
            settlement_minor: 501,
            currency: "USD".into(),
            pricing_catalog_version: "v1".into(),
            created_at: "2026-08-05T20:02:00Z".into(),
            settled_at: None,
            hubu_settlement_id: None,
        };
        assert!(matches!(
            r.create_receipt(&receipt),
            Err(Error::OverAuthorization)
        ));
        let other = r.create_execution(&new("b", "other")).unwrap();
        let cross_artifact = CreateArtifactParams {
            execution_id: other.execution_id.clone(),
            provider_attempt_id: Some(receipt.provider_attempt_id.clone()),
            kind: "image".into(),
            media_type: "image/png".into(),
            storage_key: "objects/cross-aggregate".into(),
            byte_size: 1,
            sha256: "cross".into(),
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
            execution_id: other.execution_id,
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
}
