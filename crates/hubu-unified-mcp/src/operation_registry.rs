use std::{fs, path::Path, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Deserialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const CONTROLLED_INVOCATION_META_KEY: &str = "hubu.dev/platform-invocation";

const CODEX_CALL_ID_KEY: &str = "callId";
const CLAUDE_TOOL_USE_ID_KEY: &str = "claudecode/toolUseId";
const MAX_PLATFORM_BYTES: usize = 64;
const MAX_HARNESS_ID_BYTES: usize = 512;
const MAX_TASK_ID_BYTES: usize = 512;
const MAX_TOOL_NAME_BYTES: usize = 128;
const SCHEMA_VERSION: i64 = 2;
const APPLICATION_ID: i64 = 0x4855_424f;
const MAX_RESULT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NormalizedHarnessIdentity {
    platform: String,
    harness_call_id: String,
    codex_call_id: Option<String>,
    claude_tool_use_id: Option<String>,
    hubu_invocation_id: Option<String>,
    controlled_installation_id: Option<String>,
    task_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlledInvocationEnvelope {
    platform: String,
    invocation_id: String,
    #[serde(default)]
    installation_id: Option<String>,
    #[serde(default)]
    task_id: Option<String>,
}

impl NormalizedHarnessIdentity {
    pub(crate) fn from_meta(meta: Option<&Value>) -> Result<Self> {
        let metadata = meta
            .and_then(Value::as_object)
            .ok_or_else(|| anyhow!("Hubu spend tools require trusted harness call identity"))?;
        let sources = [
            metadata.get(CODEX_CALL_ID_KEY).is_some(),
            metadata.get(CLAUDE_TOOL_USE_ID_KEY).is_some(),
            metadata.get(CONTROLLED_INVOCATION_META_KEY).is_some(),
        ];
        match sources.into_iter().filter(|present| *present).count() {
            0 => bail!(
                "Hubu spend tools require supported trusted identity from Codex, Claude Code, or the Hubu platform-invocation envelope"
            ),
            1 => {}
            _ => bail!("Hubu spend tool identity is ambiguous across multiple harness adapters"),
        }

        let identity = if let Some(value) = metadata.get(CODEX_CALL_ID_KEY) {
            let call_id = required_string(value, "Codex callId")?;
            Self {
                platform: "codex".to_owned(),
                harness_call_id: call_id.clone(),
                codex_call_id: Some(call_id),
                claude_tool_use_id: None,
                hubu_invocation_id: None,
                controlled_installation_id: None,
                task_id: None,
            }
        } else if let Some(value) = metadata.get(CLAUDE_TOOL_USE_ID_KEY) {
            let tool_use_id = required_string(value, "Claude Code toolUseId")?;
            Self {
                platform: "claude-code".to_owned(),
                harness_call_id: tool_use_id.clone(),
                codex_call_id: None,
                claude_tool_use_id: Some(tool_use_id),
                hubu_invocation_id: None,
                controlled_installation_id: None,
                task_id: None,
            }
        } else {
            let envelope: ControlledInvocationEnvelope =
                serde_json::from_value(metadata[CONTROLLED_INVOCATION_META_KEY].clone())
                    .context("trusted Hubu platform-invocation metadata is invalid")?;
            Self {
                platform: envelope.platform,
                harness_call_id: envelope.invocation_id.clone(),
                codex_call_id: None,
                claude_tool_use_id: None,
                hubu_invocation_id: Some(envelope.invocation_id),
                controlled_installation_id: envelope.installation_id,
                task_id: envelope.task_id,
            }
        };
        identity.validate()?;
        Ok(identity)
    }

    fn validate(&self) -> Result<()> {
        validate_platform(&self.platform)?;
        validate_identifier(
            "harness_call_id",
            &self.harness_call_id,
            MAX_HARNESS_ID_BYTES,
        )?;
        for (name, value) in [
            ("codex_call_id", self.codex_call_id.as_deref()),
            ("claude_tool_use_id", self.claude_tool_use_id.as_deref()),
            ("hubu_invocation_id", self.hubu_invocation_id.as_deref()),
            (
                "controlled_installation_id",
                self.controlled_installation_id.as_deref(),
            ),
        ] {
            if let Some(value) = value {
                validate_identifier(name, value, MAX_HARNESS_ID_BYTES)?;
            }
        }
        if let Some(task_id) = self.task_id.as_deref() {
            validate_identifier("task_id", task_id, MAX_TASK_ID_BYTES)?;
        }
        Ok(())
    }
}

fn required_string(value: &Value, field: &str) -> Result<String> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("trusted {field} must be a string"))
}

fn validate_platform(platform: &str) -> Result<()> {
    if platform.is_empty()
        || platform.len() > MAX_PLATFORM_BYTES
        || !platform
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        bail!(
            "trusted invocation platform must contain 1 to {MAX_PLATFORM_BYTES} ASCII letters, numbers, '.', '_', or '-'"
        );
    }
    Ok(())
}

fn validate_identifier(field: &str, value: &str, maximum: usize) -> Result<()> {
    if value.is_empty()
        || value.len() > maximum
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        bail!("trusted invocation {field} must be 1 to {maximum} bytes without surrounding whitespace or control characters");
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OperationResolution {
    pub(crate) operation_key: Option<String>,
    pub(crate) operation_handle: String,
    pub(crate) task_id: Option<String>,
    pub(crate) recorded_result: Option<Value>,
}

#[derive(Debug)]
struct PersistedOperation {
    request_hash: String,
    operation_key: Option<String>,
    operation_handle: String,
    codex_call_id: Option<String>,
    claude_tool_use_id: Option<String>,
    hubu_invocation_id: Option<String>,
    task_id: Option<String>,
    decision: Option<String>,
    result_json: Option<String>,
}

pub(crate) struct OperationRegistry {
    connection: Connection,
    installation_id: String,
}

impl std::fmt::Debug for OperationRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OperationRegistry")
            .field("installation_id", &"<persisted>")
            .finish_non_exhaustive()
    }
}

impl OperationRegistry {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        if path == Path::new(":memory:") {
            bail!("unified MCP operation registry requires a persistent on-disk path; in-memory state is test-only");
        }
        if !path.is_absolute() {
            bail!("unified MCP operation registry path must be absolute");
        }
        if let Ok(metadata) = fs::symlink_metadata(path) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                bail!("unified MCP operation registry path must name a regular file");
            }
        }
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or_else(|| anyhow!("unified MCP operation registry path has no parent"))?;
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "create unified MCP operation registry directory `{}`",
                parent.display()
            )
        })?;

        let connection = Connection::open(path)
            .with_context(|| format!("open unified MCP operation registry `{}`", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).with_context(|| {
                format!("secure unified MCP operation registry `{}`", path.display())
            })?;
        }
        Self::from_connection(connection)
    }

    fn from_connection(mut connection: Connection) -> Result<Self> {
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch("PRAGMA foreign_keys = ON;")?;
        let application_id =
            connection.query_row("PRAGMA application_id", [], |row| row.get::<_, i64>(0))?;
        let version =
            connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?;
        let user_table_count = connection.query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        if application_id == 0 && version == 0 && user_table_count == 0 {
            create_schema(&connection)?;
        } else if application_id == APPLICATION_ID && version == 1 {
            migrate_v1_to_v2(&mut connection)?;
        } else if application_id != APPLICATION_ID || version != SCHEMA_VERSION {
            bail!("unified MCP operation registry identity or schema version is unsupported; refusing to modify the configured database");
        }
        validate_schema(&connection)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let installation_id = transaction
            .query_row(
                "SELECT installation_id FROM installation_identity WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(|| format!("hubu-installation:v1:{}", Uuid::new_v4().simple()));
        transaction.execute(
            "INSERT OR IGNORE INTO installation_identity(singleton, installation_id) VALUES (1, ?1)",
            [&installation_id],
        )?;
        let installation_id = transaction.query_row(
            "SELECT installation_id FROM installation_identity WHERE singleton = 1",
            [],
            |row| row.get::<_, String>(0),
        )?;
        transaction.commit()?;
        validate_identifier("installation_id", &installation_id, 128)?;
        let mut registry = Self {
            connection,
            installation_id,
        };
        registry.remove_expired_authorization_identifiers()?;
        Ok(registry)
    }

    #[cfg(test)]
    pub(crate) fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    pub(crate) fn resolve_or_allocate(
        &mut self,
        identity: &NormalizedHarnessIdentity,
        tool_name: &str,
        arguments: &Value,
    ) -> Result<OperationResolution> {
        self.remove_expired_authorization_identifiers()?;
        identity.validate()?;
        validate_identifier("tool_name", tool_name, MAX_TOOL_NAME_BYTES)?;
        if !arguments.is_object() {
            bail!("Hubu spend tool arguments must be an object");
        }
        let request_hash = canonical_request_hash(tool_name, arguments)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = transaction
            .query_row(
                "SELECT request_hash, operation_key, operation_handle, codex_call_id,
                        claude_tool_use_id, hubu_invocation_id, task_id, decision,
                        result_json
                 FROM harness_operations
                 WHERE platform = ?1 AND installation_id = ?2 AND harness_call_id = ?3",
                params![
                    identity.platform,
                    self.installation_id,
                    identity.harness_call_id
                ],
                |row| {
                    Ok(PersistedOperation {
                        request_hash: row.get(0)?,
                        operation_key: row.get(1)?,
                        operation_handle: row.get(2)?,
                        codex_call_id: row.get(3)?,
                        claude_tool_use_id: row.get(4)?,
                        hubu_invocation_id: row.get(5)?,
                        task_id: row.get(6)?,
                        decision: row.get(7)?,
                        result_json: row.get(8)?,
                    })
                },
            )
            .optional()?;
        if let Some(existing) = existing {
            if existing.request_hash != request_hash
                || existing.codex_call_id != identity.codex_call_id
                || existing.claude_tool_use_id != identity.claude_tool_use_id
                || existing.hubu_invocation_id != identity.hubu_invocation_id
                || existing.task_id != identity.task_id
            {
                bail!("trusted harness call identity was already used for a different operation; refusing backend access");
            }
            transaction.commit()?;
            let recorded_result = if matches!(existing.decision.as_deref(), Some("allow" | "deny"))
            {
                existing
                    .result_json
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()
                    .context("decode recorded normalized operation result")?
            } else {
                None
            };
            return Ok(OperationResolution {
                operation_key: existing.operation_key,
                operation_handle: existing.operation_handle,
                task_id: existing.task_id,
                recorded_result,
            });
        }

        let operation_key = format!(
            "hubu:operation:v1:{}:{}",
            identity.platform,
            Uuid::new_v4().simple()
        );
        let operation_handle = format!("hubu:public-operation:v1:{}", Uuid::new_v4().simple());
        transaction.execute(
            "INSERT INTO harness_operations (
                 platform, installation_id, harness_call_id, request_hash, operation_key,
                 operation_handle,
                 codex_call_id, claude_tool_use_id, hubu_invocation_id,
                 controlled_installation_id, task_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                identity.platform,
                self.installation_id,
                identity.harness_call_id,
                request_hash,
                operation_key,
                operation_handle,
                identity.codex_call_id,
                identity.claude_tool_use_id,
                identity.hubu_invocation_id,
                identity.controlled_installation_id,
                identity.task_id,
            ],
        )?;
        transaction.commit()?;
        Ok(OperationResolution {
            operation_key: Some(operation_key),
            operation_handle,
            task_id: identity.task_id.clone(),
            recorded_result: None,
        })
    }

    pub(crate) fn mark_dispatch_started(&mut self, operation_handle: &str) -> Result<()> {
        let changed = self.connection.execute(
            "UPDATE harness_operations
             SET dispatch_started_at = COALESCE(dispatch_started_at, CURRENT_TIMESTAMP)
             WHERE operation_handle = ?1 AND operation_key IS NOT NULL",
            [operation_handle],
        )?;
        if changed != 1 {
            bail!("normalized operation cannot be dispatched");
        }
        Ok(())
    }

    pub(crate) fn record_authorization_result(
        &mut self,
        operation_handle: &str,
        result: &Value,
    ) -> Result<Value> {
        if contains_protected_identity(result) {
            bail!("normalized operation result contains protected backend identity");
        }
        let result_json = serde_json::to_string(result)?;
        if result_json.len() > MAX_RESULT_BYTES {
            bail!("Hubu spend authorization result is too large to persist safely");
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (private_operation_key, existing_decision, existing_result) = transaction
            .query_row(
                "SELECT operation_key, decision, result_json
                 FROM harness_operations WHERE operation_handle = ?1",
                [operation_handle],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("normalized operation result has no matching operation"))?;
        if private_operation_key
            .as_deref()
            .is_some_and(|operation_key| result_json.contains(operation_key))
        {
            bail!("normalized operation result contains private backend identity");
        }
        let decision = result
            .get("decision")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Hubu spend authorization result is missing decision"))?;
        if !matches!(decision, "allow" | "deny" | "needs_approval") {
            bail!("Hubu spend authorization result has an unsupported decision");
        }
        if matches!(existing_decision.as_deref(), Some("allow" | "deny")) {
            let existing_result = existing_result
                .as_deref()
                .ok_or_else(|| anyhow!("terminal normalized operation is missing replay state"))
                .and_then(|value| {
                    serde_json::from_str(value)
                        .context("decode recorded normalized operation result")
                })?;
            transaction.commit()?;
            return Ok(existing_result);
        }
        let decision_id = optional_string(result, "decision_id")?;
        let auth_token_id = optional_string(result, "auth_token_id")?
            .or(optional_string(result, "spend_auth_token_id")?);
        let approval_request_id = result
            .get("approval")
            .and_then(|approval| approval.get("approval_request_id"))
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| anyhow!("approval_request_id must be a string"))
            })
            .transpose()?;
        let authorization_expires_at = optional_string(result, "authorization_expires_at")?;
        let terminal = matches!(decision, "allow" | "deny");
        let changed = transaction.execute(
            "UPDATE harness_operations
             SET decision = ?2,
                 decision_id = ?3,
                 auth_token_id = ?4,
                 approval_request_id = ?5,
                 authorization_expires_at = ?6,
                 result_json = ?7,
                 result_recorded_at = CURRENT_TIMESTAMP,
                 operation_key = CASE WHEN ?8 THEN NULL ELSE operation_key END
             WHERE operation_handle = ?1",
            params![
                operation_handle,
                decision,
                decision_id,
                auth_token_id,
                approval_request_id,
                authorization_expires_at,
                result_json,
                terminal,
            ],
        )?;
        if changed != 1 {
            bail!("normalized operation result has no matching operation");
        }
        transaction.commit()?;
        Ok(result.clone())
    }

    fn remove_expired_authorization_identifiers(&mut self) -> Result<()> {
        let now = Utc::now();
        let rows = {
            let mut statement = self.connection.prepare(
                "SELECT operation_handle, authorization_expires_at, result_json
                 FROM harness_operations
                 WHERE auth_token_id IS NOT NULL AND authorization_expires_at IS NOT NULL",
            )?;
            let mapped = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?;
            mapped.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (handle, expires_at, result_json) in rows {
            let Ok(expires_at) = DateTime::parse_from_rfc3339(&expires_at) else {
                continue;
            };
            if expires_at.with_timezone(&Utc) > now {
                continue;
            }
            let result_json = result_json
                .map(|value| -> Result<String> {
                    let mut value: Value = serde_json::from_str(&value)?;
                    if let Some(object) = value.as_object_mut() {
                        object.remove("auth_token_id");
                        object.remove("spend_auth_token_id");
                    }
                    Ok(serde_json::to_string(&value)?)
                })
                .transpose()?;
            self.connection.execute(
                "UPDATE harness_operations
                 SET auth_token_id = NULL, result_json = ?2
                 WHERE operation_handle = ?1",
                params![handle, result_json],
            )?;
        }
        Ok(())
    }
}

fn contains_protected_identity(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().any(contains_protected_identity),
        Value::Object(object) => {
            object.contains_key("operation_key") || object.values().any(contains_protected_identity)
        }
        _ => false,
    }
}

fn create_schema(connection: &Connection) -> Result<()> {
    connection.execute_batch(&format!(
        "BEGIN IMMEDIATE;
         CREATE TABLE installation_identity (
             singleton INTEGER NOT NULL PRIMARY KEY CHECK(singleton = 1),
             installation_id TEXT NOT NULL UNIQUE CHECK(length(installation_id) BETWEEN 1 AND 128),
             created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
         );
         CREATE TABLE harness_operations (
             platform TEXT NOT NULL CHECK(length(platform) BETWEEN 1 AND 64),
             installation_id TEXT NOT NULL CHECK(length(installation_id) BETWEEN 1 AND 128),
             harness_call_id TEXT NOT NULL CHECK(length(harness_call_id) BETWEEN 1 AND 512),
             request_hash TEXT NOT NULL CHECK(length(request_hash) = 71),
             operation_key TEXT UNIQUE CHECK(operation_key IS NULL OR length(operation_key) BETWEEN 1 AND 160),
             operation_handle TEXT NOT NULL UNIQUE CHECK(length(operation_handle) BETWEEN 1 AND 160),
             codex_call_id TEXT CHECK(codex_call_id IS NULL OR length(codex_call_id) BETWEEN 1 AND 512),
             claude_tool_use_id TEXT CHECK(claude_tool_use_id IS NULL OR length(claude_tool_use_id) BETWEEN 1 AND 512),
             hubu_invocation_id TEXT CHECK(hubu_invocation_id IS NULL OR length(hubu_invocation_id) BETWEEN 1 AND 512),
             controlled_installation_id TEXT CHECK(controlled_installation_id IS NULL OR length(controlled_installation_id) BETWEEN 1 AND 512),
             task_id TEXT CHECK(task_id IS NULL OR length(task_id) BETWEEN 1 AND 512),
             decision TEXT CHECK(decision IS NULL OR decision IN ('allow', 'deny', 'needs_approval')),
             decision_id TEXT,
             auth_token_id TEXT,
             approval_request_id TEXT,
             authorization_expires_at TEXT,
             result_json TEXT,
             dispatch_started_at TEXT,
             result_recorded_at TEXT,
             created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
             PRIMARY KEY(platform, installation_id, harness_call_id),
             FOREIGN KEY(installation_id) REFERENCES installation_identity(installation_id)
         );
         PRAGMA application_id = {APPLICATION_ID};
         PRAGMA user_version = {SCHEMA_VERSION};
         COMMIT;"
    ))?;
    Ok(())
}

fn migrate_v1_to_v2(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let application_id =
        transaction.query_row("PRAGMA application_id", [], |row| row.get::<_, i64>(0))?;
    let version = transaction.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?;
    if application_id == APPLICATION_ID && version == SCHEMA_VERSION {
        transaction.commit()?;
        return Ok(());
    }
    if application_id != APPLICATION_ID || version != 1 {
        bail!("unified MCP operation registry identity or schema version changed during migration");
    }
    #[allow(clippy::type_complexity)]
    let existing = {
        let mut statement = transaction.prepare(
            "SELECT platform, installation_id, harness_call_id, request_hash, operation_key,
                    codex_call_id, claude_tool_use_id, hubu_invocation_id,
                    controlled_installation_id, task_id, created_at
             FROM harness_operations",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, String>(10)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    transaction.execute_batch(
        "ALTER TABLE harness_operations RENAME TO harness_operations_v1;
         CREATE TABLE harness_operations (
             platform TEXT NOT NULL CHECK(length(platform) BETWEEN 1 AND 64),
             installation_id TEXT NOT NULL CHECK(length(installation_id) BETWEEN 1 AND 128),
             harness_call_id TEXT NOT NULL CHECK(length(harness_call_id) BETWEEN 1 AND 512),
             request_hash TEXT NOT NULL CHECK(length(request_hash) = 71),
             operation_key TEXT UNIQUE CHECK(operation_key IS NULL OR length(operation_key) BETWEEN 1 AND 160),
             operation_handle TEXT NOT NULL UNIQUE CHECK(length(operation_handle) BETWEEN 1 AND 160),
             codex_call_id TEXT CHECK(codex_call_id IS NULL OR length(codex_call_id) BETWEEN 1 AND 512),
             claude_tool_use_id TEXT CHECK(claude_tool_use_id IS NULL OR length(claude_tool_use_id) BETWEEN 1 AND 512),
             hubu_invocation_id TEXT CHECK(hubu_invocation_id IS NULL OR length(hubu_invocation_id) BETWEEN 1 AND 512),
             controlled_installation_id TEXT CHECK(controlled_installation_id IS NULL OR length(controlled_installation_id) BETWEEN 1 AND 512),
             task_id TEXT CHECK(task_id IS NULL OR length(task_id) BETWEEN 1 AND 512),
             decision TEXT CHECK(decision IS NULL OR decision IN ('allow', 'deny', 'needs_approval')),
             decision_id TEXT,
             auth_token_id TEXT,
             approval_request_id TEXT,
             authorization_expires_at TEXT,
             result_json TEXT,
             dispatch_started_at TEXT,
             result_recorded_at TEXT,
             created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
             PRIMARY KEY(platform, installation_id, harness_call_id),
             FOREIGN KEY(installation_id) REFERENCES installation_identity(installation_id)
         );",
    )?;
    for row in existing {
        transaction.execute(
            "INSERT INTO harness_operations (
                 platform, installation_id, harness_call_id, request_hash, operation_key,
                 operation_handle, codex_call_id, claude_tool_use_id, hubu_invocation_id,
                 controlled_installation_id, task_id, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                row.0,
                row.1,
                row.2,
                row.3,
                row.4,
                format!("hubu:public-operation:v1:{}", Uuid::new_v4().simple()),
                row.5,
                row.6,
                row.7,
                row.8,
                row.9,
                row.10,
            ],
        )?;
    }
    transaction.execute_batch(&format!(
        "DROP TABLE harness_operations_v1;
         PRAGMA user_version = {SCHEMA_VERSION};"
    ))?;
    transaction.commit()?;
    Ok(())
}

fn optional_string(value: &Value, field: &str) -> Result<Option<String>> {
    value
        .get(field)
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("{field} must be a string"))
        })
        .transpose()
}

fn validate_schema(connection: &Connection) -> Result<()> {
    connection
        .prepare("SELECT singleton, installation_id, created_at FROM installation_identity LIMIT 0")
        .context("validate unified MCP operation registry installation schema")?;
    connection
        .prepare(
            "SELECT platform, installation_id, harness_call_id, request_hash, operation_key,
                    operation_handle,
                    codex_call_id, claude_tool_use_id, hubu_invocation_id,
                    controlled_installation_id, task_id, decision, decision_id,
                    auth_token_id, approval_request_id, authorization_expires_at,
                    result_json, dispatch_started_at, result_recorded_at, created_at
             FROM harness_operations LIMIT 0",
        )
        .context("validate unified MCP operation registry operation schema")?;
    Ok(())
}

fn canonical_request_hash(tool_name: &str, arguments: &Value) -> Result<String> {
    let canonical_arguments = canonicalize(arguments);
    let mut digest = Sha256::new();
    digest.update(tool_name.as_bytes());
    digest.update([0]);
    digest.update(serde_json::to_vec(&canonical_arguments)?);
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonicalize).collect()),
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let mut canonical = Map::new();
            for key in keys {
                canonical.insert(key.clone(), canonicalize(&values[key]));
            }
            Value::Object(canonical)
        }
        value => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Barrier, thread};

    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    fn codex(call_id: &str) -> NormalizedHarnessIdentity {
        NormalizedHarnessIdentity::from_meta(Some(&json!({"callId": call_id}))).unwrap()
    }

    #[test]
    fn all_supported_adapters_normalize_to_typed_aliases() {
        let codex =
            NormalizedHarnessIdentity::from_meta(Some(&json!({"callId": "call-1"}))).unwrap();
        assert_eq!(codex.platform, "codex");
        assert_eq!(codex.codex_call_id.as_deref(), Some("call-1"));

        let claude =
            NormalizedHarnessIdentity::from_meta(Some(&json!({"claudecode/toolUseId": "toolu_1"})))
                .unwrap();
        assert_eq!(claude.platform, "claude-code");
        assert_eq!(claude.claude_tool_use_id.as_deref(), Some("toolu_1"));

        let hubu = NormalizedHarnessIdentity::from_meta(Some(&json!({
            CONTROLLED_INVOCATION_META_KEY: {
                "platform": "hubu-platform",
                "invocation_id": "invocation-1",
                "installation_id": "diagnostic-installation-1",
                "task_id": "HUB-124"
            }
        })))
        .unwrap();
        assert_eq!(hubu.platform, "hubu-platform");
        assert_eq!(hubu.hubu_invocation_id.as_deref(), Some("invocation-1"));
        assert_eq!(
            hubu.controlled_installation_id.as_deref(),
            Some("diagnostic-installation-1")
        );
        assert_eq!(hubu.task_id.as_deref(), Some("HUB-124"));
    }

    #[test]
    fn unsupported_missing_ambiguous_and_raw_key_identity_fail_closed() {
        assert!(NormalizedHarnessIdentity::from_meta(None).is_err());
        assert!(NormalizedHarnessIdentity::from_meta(Some(&json!({"unknown": "call"}))).is_err());
        assert!(NormalizedHarnessIdentity::from_meta(Some(&json!({
            "callId": "call-1",
            "claudecode/toolUseId": "toolu_1"
        })))
        .is_err());
        let error = NormalizedHarnessIdentity::from_meta(Some(&json!({
            CONTROLLED_INVOCATION_META_KEY: {
                "platform": "hubu",
                "invocation_id": "invocation-1",
                "operation_key": "client-owned"
            }
        })))
        .unwrap_err()
        .to_string();
        assert!(error.contains("invalid"));
        assert!(!error.contains("client-owned"));
    }

    #[test]
    fn exact_redelivery_reuses_and_collision_rejects() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let first = registry
            .resolve_or_allocate(
                &codex("call-1"),
                "hubu_submit_spend",
                &json!({"b": 2, "a": 1}),
            )
            .unwrap();
        let retry = registry
            .resolve_or_allocate(
                &codex("call-1"),
                "hubu_submit_spend",
                &json!({"a": 1, "b": 2}),
            )
            .unwrap();
        assert_eq!(retry, first);
        assert!(first
            .operation_key
            .as_deref()
            .unwrap()
            .starts_with("hubu:operation:v1:codex:"));

        let error = registry
            .resolve_or_allocate(
                &codex("call-1"),
                "hubu_submit_spend",
                &json!({"a": 2, "b": 2}),
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("refusing backend access"));
    }

    #[test]
    fn distinct_calls_allocate_distinct_operations() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let first = registry
            .resolve_or_allocate(&codex("call-1"), "hubu_submit_spend", &json!({"a": 1}))
            .unwrap();
        let second = registry
            .resolve_or_allocate(&codex("call-2"), "hubu_submit_spend", &json!({"a": 1}))
            .unwrap();
        assert_ne!(first.operation_key, second.operation_key);
    }

    #[test]
    fn controlled_installation_alias_is_diagnostic_not_dedup_authority() {
        let identity = |installation_id: &str| {
            NormalizedHarnessIdentity::from_meta(Some(&json!({
                CONTROLLED_INVOCATION_META_KEY: {
                    "platform": "hubu-platform",
                    "invocation_id": "same-call",
                    "installation_id": installation_id
                }
            })))
            .unwrap()
        };
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let first = registry
            .resolve_or_allocate(
                &identity("diagnostic-a"),
                "hubu_submit_spend",
                &json!({"a": 1}),
            )
            .unwrap();
        let redelivery = registry
            .resolve_or_allocate(
                &identity("diagnostic-b"),
                "hubu_submit_spend",
                &json!({"a": 1}),
            )
            .unwrap();
        assert_eq!(redelivery, first);
    }

    #[test]
    fn restart_recovers_installation_and_operation_identity() {
        let root = tempdir().unwrap();
        let path = root.path().join("operations.sqlite3");
        let first = OperationRegistry::open(&path)
            .unwrap()
            .resolve_or_allocate(&codex("restart"), "hubu_authorize_spend", &json!({"a": 1}))
            .unwrap();
        let recovered = OperationRegistry::open(&path)
            .unwrap()
            .resolve_or_allocate(&codex("restart"), "hubu_authorize_spend", &json!({"a": 1}))
            .unwrap();
        assert_eq!(recovered, first);
    }

    #[test]
    fn terminal_result_replays_without_private_key_and_pending_result_remains_dispatchable() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let pending = registry
            .resolve_or_allocate(
                &codex("lifecycle"),
                "hubu_authorize_spend",
                &json!({"amount": 1}),
            )
            .unwrap();
        registry
            .mark_dispatch_started(&pending.operation_handle)
            .unwrap();
        registry
            .record_authorization_result(
                &pending.operation_handle,
                &json!({
                    "decision":"needs_approval",
                    "decision_id":"decision-1",
                    "approval":{"approval_request_id":"approval-1"},
                    "operation_handle":pending.operation_handle
                }),
            )
            .unwrap();
        let recovered_pending = registry
            .resolve_or_allocate(
                &codex("lifecycle"),
                "hubu_authorize_spend",
                &json!({"amount": 1}),
            )
            .unwrap();
        assert!(recovered_pending.operation_key.is_some());
        assert!(recovered_pending.recorded_result.is_none());

        let terminal = json!({
            "decision":"allow",
            "decision_id":"decision-1",
            "auth_token_id":"authorization-1",
            "operation_handle":pending.operation_handle
        });
        registry
            .record_authorization_result(&pending.operation_handle, &terminal)
            .unwrap();
        let recovered_terminal = registry
            .resolve_or_allocate(
                &codex("lifecycle"),
                "hubu_authorize_spend",
                &json!({"amount": 1}),
            )
            .unwrap();
        assert!(recovered_terminal.operation_key.is_none());
        assert_eq!(recovered_terminal.recorded_result, Some(terminal));
        let stored: (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = registry
            .connection
            .query_row(
                "SELECT operation_key, decision_id, auth_token_id, approval_request_id
                 FROM harness_operations WHERE operation_handle = ?1",
                [&pending.operation_handle],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            stored,
            (
                None,
                Some("decision-1".into()),
                Some("authorization-1".into()),
                None
            )
        );
    }

    #[test]
    fn terminal_result_is_monotonic_against_a_delayed_pending_response() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let operation = registry
            .resolve_or_allocate(
                &codex("monotonic"),
                "hubu_authorize_spend",
                &json!({"amount": 1}),
            )
            .unwrap();
        let terminal = json!({
            "decision":"allow",
            "decision_id":"terminal-decision",
            "operation_handle":operation.operation_handle
        });
        assert_eq!(
            registry
                .record_authorization_result(&operation.operation_handle, &terminal)
                .unwrap(),
            terminal
        );

        let delayed = json!({
            "decision":"needs_approval",
            "decision_id":"stale-decision",
            "operation_handle":operation.operation_handle
        });
        assert_eq!(
            registry
                .record_authorization_result(&operation.operation_handle, &delayed)
                .unwrap(),
            terminal
        );
        let recovered = registry
            .resolve_or_allocate(
                &codex("monotonic"),
                "hubu_authorize_spend",
                &json!({"amount": 1}),
            )
            .unwrap();
        assert!(recovered.operation_key.is_none());
        assert_eq!(recovered.recorded_result, Some(terminal));
    }

    #[test]
    fn private_operation_key_is_rejected_from_recorded_result() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let operation = registry
            .resolve_or_allocate(&codex("protected"), "hubu_submit_spend", &json!({"a": 1}))
            .unwrap();
        for result in [
            json!({"decision":"allow","operation_key":"private"}),
            json!({"decision":"deny","nested":{"operation_key":"private"}}),
            json!({
                "decision":"allow",
                "reason":operation.operation_key.as_deref().unwrap()
            }),
        ] {
            assert!(registry
                .record_authorization_result(&operation.operation_handle, &result)
                .unwrap_err()
                .to_string()
                .contains("backend identity"));
        }
    }

    #[test]
    fn expired_authorization_identifier_is_removed_on_restart() {
        let root = tempdir().unwrap();
        let path = root.path().join("operations.sqlite3");
        let mut registry = OperationRegistry::open(&path).unwrap();
        let operation = registry
            .resolve_or_allocate(&codex("expired"), "hubu_authorize_spend", &json!({"a": 1}))
            .unwrap();
        registry
            .record_authorization_result(
                &operation.operation_handle,
                &json!({
                    "decision":"allow",
                    "auth_token_id":"expired-authorization",
                    "authorization_expires_at":"2020-01-01T00:00:00Z",
                    "operation_handle":operation.operation_handle
                }),
            )
            .unwrap();
        drop(registry);

        let mut restarted = OperationRegistry::open(&path).unwrap();
        let recovered = restarted
            .resolve_or_allocate(&codex("expired"), "hubu_authorize_spend", &json!({"a": 1}))
            .unwrap();
        assert!(recovered
            .recorded_result
            .unwrap()
            .get("auth_token_id")
            .is_none());
        let stored: Option<String> = restarted
            .connection
            .query_row(
                "SELECT auth_token_id FROM harness_operations WHERE operation_handle = ?1",
                [&operation.operation_handle],
                |row| row.get(0),
            )
            .unwrap();
        assert!(stored.is_none());
    }

    #[test]
    fn expired_authorization_identifier_is_removed_before_live_replay() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let operation = registry
            .resolve_or_allocate(
                &codex("expired-live"),
                "hubu_authorize_spend",
                &json!({"a": 1}),
            )
            .unwrap();
        registry
            .record_authorization_result(
                &operation.operation_handle,
                &json!({
                    "decision":"allow",
                    "auth_token_id":"expired-authorization",
                    "authorization_expires_at":"2020-01-01T00:00:00Z",
                    "operation_handle":operation.operation_handle
                }),
            )
            .unwrap();

        let recovered = registry
            .resolve_or_allocate(
                &codex("expired-live"),
                "hubu_authorize_spend",
                &json!({"a": 1}),
            )
            .unwrap();
        assert!(recovered
            .recorded_result
            .unwrap()
            .get("auth_token_id")
            .is_none());
    }

    #[test]
    fn v1_registry_migrates_with_stable_public_handle() {
        let root = tempdir().unwrap();
        let path = root.path().join("operations.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(&format!(
                "CREATE TABLE installation_identity (
                     singleton INTEGER NOT NULL PRIMARY KEY,
                     installation_id TEXT NOT NULL UNIQUE,
                     created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                 );
                 INSERT INTO installation_identity(singleton, installation_id)
                 VALUES(1, 'installation-v1');
                 CREATE TABLE harness_operations (
                     platform TEXT NOT NULL,
                     installation_id TEXT NOT NULL,
                     harness_call_id TEXT NOT NULL,
                     request_hash TEXT NOT NULL,
                     operation_key TEXT NOT NULL UNIQUE,
                     codex_call_id TEXT,
                     claude_tool_use_id TEXT,
                     hubu_invocation_id TEXT,
                     controlled_installation_id TEXT,
                     task_id TEXT,
                     created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                     PRIMARY KEY(platform, installation_id, harness_call_id)
                 );
                 INSERT INTO harness_operations(
                     platform, installation_id, harness_call_id, request_hash,
                     operation_key, codex_call_id
                 ) VALUES(
                     'codex', 'installation-v1', 'migration-call',
                     '{}', 'hubu:operation:v1:codex:migrated', 'migration-call'
                 );
                 PRAGMA application_id = {APPLICATION_ID};
                 PRAGMA user_version = 1;",
                canonical_request_hash("hubu_submit_spend", &json!({"a": 1})).unwrap()
            ))
            .unwrap();
        drop(connection);

        let first = OperationRegistry::open(&path)
            .unwrap()
            .resolve_or_allocate(
                &codex("migration-call"),
                "hubu_submit_spend",
                &json!({"a": 1}),
            )
            .unwrap();
        let second = OperationRegistry::open(&path)
            .unwrap()
            .resolve_or_allocate(
                &codex("migration-call"),
                "hubu_submit_spend",
                &json!({"a": 1}),
            )
            .unwrap();
        assert_eq!(first, second);
        assert!(first
            .operation_handle
            .starts_with("hubu:public-operation:v1:"));
        assert_eq!(
            first.operation_key.as_deref(),
            Some("hubu:operation:v1:codex:migrated")
        );
    }

    #[cfg(unix)]
    #[test]
    fn existing_registry_file_is_hardened_on_open() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempdir().unwrap();
        let path = root.path().join("operations.sqlite3");
        fs::File::create(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        OperationRegistry::open(&path).unwrap();

        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn unrelated_sqlite_database_is_rejected_without_schema_mutation() {
        let root = tempdir().unwrap();
        let path = root.path().join("unrelated.sqlite3");
        let unrelated = Connection::open(&path).unwrap();
        unrelated
            .execute_batch("CREATE TABLE unrelated(value TEXT); PRAGMA user_version = 1;")
            .unwrap();
        drop(unrelated);

        let error = OperationRegistry::open(&path).unwrap_err().to_string();
        assert!(error.contains("refusing to modify"));

        let unrelated = Connection::open(&path).unwrap();
        assert_eq!(
            unrelated
                .query_row("PRAGMA application_id", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            unrelated
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'unrelated'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            unrelated
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'installation_identity'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn concurrent_allocation_creates_one_record() {
        let root = tempdir().unwrap();
        let path = root.path().join("operations.sqlite3");
        OperationRegistry::open(&path).unwrap();
        let barrier = std::sync::Arc::new(Barrier::new(8));
        let handles = (0..8)
            .map(|_| {
                let path = path.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    let mut registry = OperationRegistry::open(&path).unwrap();
                    barrier.wait();
                    registry
                        .resolve_or_allocate(
                            &codex("concurrent"),
                            "hubu_submit_spend",
                            &json!({"amount": 1}),
                        )
                        .unwrap()
                        .operation_key
                })
            })
            .collect::<Vec<_>>();
        let keys = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert!(keys.iter().all(|key| key == &keys[0]));
    }

    #[test]
    fn persisted_identifiers_are_bounded() {
        let too_long = "x".repeat(MAX_HARNESS_ID_BYTES + 1);
        assert!(NormalizedHarnessIdentity::from_meta(Some(&json!({"callId": too_long}))).is_err());
        assert!(
            NormalizedHarnessIdentity::from_meta(Some(&json!({"callId": " spaced "}))).is_err()
        );
        assert!(
            NormalizedHarnessIdentity::from_meta(Some(&json!({"callId": "line\nbreak"}))).is_err()
        );
    }
}
