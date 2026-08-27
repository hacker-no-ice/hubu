use std::{fs, io, path::Path, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
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
const SCHEMA_VERSION: i64 = 4;
const APPLICATION_ID: i64 = 0x4855_424f;
const MAX_RESULT_BYTES: usize = 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const WORKER_LEASE_SECONDS: i64 = 10;
const OPERATION_DEADLINE_HOURS: i64 = 24;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GongbuContinuation {
    pub(crate) operation_key: String,
    pub(crate) operation_handle: String,
    pub(crate) execution_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GongbuLifecycle {
    pub(crate) execution_id: String,
    pub(crate) operation_key: String,
    pub(crate) status: String,
    pub(crate) outcome: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DurableOperationStatus {
    pub(crate) operation_handle: String,
    pub(crate) state: String,
    pub(crate) execution_id: Option<String>,
    pub(crate) result_code: Option<String>,
    pub(crate) updated_at: String,
}

impl DurableOperationStatus {
    pub(crate) fn terminal(&self) -> bool {
        matches!(self.state.as_str(), "succeeded" | "failed")
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ClaimedDurableOperation {
    pub(crate) lease_id: String,
    pub(crate) operation_key: String,
    pub(crate) operation_handle: String,
    pub(crate) request: Option<Value>,
    pub(crate) execution_id: Option<String>,
    pub(crate) dispatch_attempts: u32,
    pub(crate) observation_failures: u32,
    pub(crate) reconciliation_attempts: u32,
    pub(crate) deadline_expired: bool,
}

#[derive(Debug)]
struct PersistedOperation {
    request_hash: String,
    tool_name: String,
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
                "SELECT request_hash, tool_name, operation_key, operation_handle, codex_call_id,
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
                        tool_name: row.get(1)?,
                        operation_key: row.get(2)?,
                        operation_handle: row.get(3)?,
                        codex_call_id: row.get(4)?,
                        claude_tool_use_id: row.get(5)?,
                        hubu_invocation_id: row.get(6)?,
                        task_id: row.get(7)?,
                        decision: row.get(8)?,
                        result_json: row.get(9)?,
                    })
                },
            )
            .optional()?;
        if let Some(existing) = existing {
            if existing.request_hash != request_hash
                || existing.tool_name != tool_name
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
                 platform, installation_id, harness_call_id, request_hash, tool_name, operation_key,
                 operation_handle,
                 codex_call_id, claude_tool_use_id, hubu_invocation_id,
                 controlled_installation_id, task_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                identity.platform,
                self.installation_id,
                identity.harness_call_id,
                request_hash,
                tool_name,
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

    pub(crate) fn mark_dispatch_started(
        &mut self,
        operation_handle: &str,
    ) -> Result<Option<Value>> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (operation_key, decision, result_json) = transaction
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
            .ok_or_else(|| anyhow!("normalized operation cannot be dispatched"))?;
        if matches!(decision.as_deref(), Some("allow" | "deny")) {
            let result = result_json
                .as_deref()
                .ok_or_else(|| anyhow!("terminal normalized operation is missing replay state"))
                .and_then(|value| {
                    serde_json::from_str(value)
                        .context("decode recorded normalized operation result")
                })?;
            transaction.commit()?;
            return Ok(Some(result));
        }
        if operation_key.is_none() {
            bail!("normalized operation cannot be dispatched");
        }
        let changed = transaction.execute(
            "UPDATE harness_operations
             SET dispatch_started_at = COALESCE(dispatch_started_at, CURRENT_TIMESTAMP)
             WHERE operation_handle = ?1 AND operation_key IS NOT NULL",
            [operation_handle],
        )?;
        if changed != 1 {
            bail!("normalized operation cannot be dispatched");
        }
        transaction.commit()?;
        Ok(None)
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
        let changed = transaction.execute(
            "UPDATE harness_operations
             SET decision = ?2,
                 decision_id = ?3,
                 auth_token_id = ?4,
                 approval_request_id = ?5,
                 authorization_expires_at = ?6,
                 result_json = ?7,
                 result_recorded_at = CURRENT_TIMESTAMP
             WHERE operation_handle = ?1",
            params![
                operation_handle,
                decision,
                decision_id,
                auth_token_id,
                approval_request_id,
                authorization_expires_at,
                result_json,
            ],
        )?;
        if changed != 1 {
            bail!("normalized operation result has no matching operation");
        }
        transaction.commit()?;
        Ok(result.clone())
    }

    pub(crate) fn resolve_gongbu_continuation(
        &mut self,
        auth_token_id: &str,
        arguments: &Value,
    ) -> Result<GongbuContinuation> {
        self.remove_expired_authorization_identifiers()?;
        validate_identifier("auth_token_id", auth_token_id, 255)?;
        if !arguments.is_object() {
            bail!("Gongbu execution arguments must be an object");
        }
        validate_gongbu_request_size(arguments)?;
        let request_hash = canonical_request_hash("gongbu_create_execution", arguments)?;
        let request_json = serde_json::to_string(&canonicalize(arguments))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let continuation = transaction
            .query_row(
                "SELECT operation_key, operation_handle, tool_name, decision,
                        gongbu_request_hash, gongbu_execution_id
                 FROM harness_operations WHERE auth_token_id = ?1",
                [auth_token_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("authorization continuation is unknown or expired"))?;
        let operation_key = continuation.0.ok_or_else(|| {
            anyhow!("authorization continuation is missing private operation identity")
        })?;
        if !matches!(
            continuation.2.as_str(),
            "hubu_authorize_spend" | "hubu_submit_governed_execution"
        ) {
            bail!("authorization continuation does not belong to an execution authorization");
        }
        if continuation.3.as_deref() != Some("allow") {
            bail!("authorization continuation is not executable");
        }
        if continuation
            .4
            .as_deref()
            .is_some_and(|existing| existing != request_hash)
        {
            bail!("authorization continuation was already bound to different execution intent; refusing backend access");
        }
        let changed = transaction.execute(
             "UPDATE harness_operations
             SET gongbu_request_hash = COALESCE(gongbu_request_hash, ?2),
                 gongbu_request_json = CASE
                     WHEN operation_state IN ('succeeded','failed') OR gongbu_execution_id IS NOT NULL
                         THEN gongbu_request_json
                     ELSE COALESCE(gongbu_request_json, ?3)
                 END,
                 operation_state = COALESCE(operation_state, 'accepted'),
                 operation_deadline_at = COALESCE(
                     operation_deadline_at,
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+' || ?4 || ' hours')
                 ),
                 operation_updated_at = CURRENT_TIMESTAMP,
                 gongbu_create_started_at = COALESCE(gongbu_create_started_at, CURRENT_TIMESTAMP)
             WHERE operation_handle = ?1
               AND (gongbu_request_hash IS NULL OR gongbu_request_hash = ?2)",
            params![
                continuation.1,
                request_hash,
                request_json,
                OPERATION_DEADLINE_HOURS
            ],
        )?;
        if changed != 1 {
            bail!("authorization continuation identity conflict; refusing backend access");
        }
        transaction.commit()?;
        Ok(GongbuContinuation {
            operation_key,
            operation_handle: continuation.1,
            execution_id: continuation.5,
        })
    }

    pub(crate) fn durable_operation_status(
        &self,
        operation_handle: &str,
    ) -> Result<DurableOperationStatus> {
        validate_public_operation_handle(operation_handle)?;
        let persisted = self
            .connection
            .query_row(
                "SELECT operation_handle, operation_state, gongbu_execution_id,
                        operation_result_code, tool_name, decision, auth_token_id,
                        authorization_expires_at,
                        COALESCE(operation_updated_at, result_recorded_at,
                                 dispatch_started_at, created_at)
                 FROM harness_operations WHERE operation_handle = ?1",
                [operation_handle],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("public operation handle is unknown"))?;
        let (state, result_code) = match persisted.1 {
            Some(state) => (state, persisted.3),
            None => pre_execution_projection(
                &persisted.4,
                persisted.5.as_deref(),
                authorization_is_live(persisted.6.as_deref(), persisted.7.as_deref()),
            )?,
        };
        Ok(DurableOperationStatus {
            operation_handle: persisted.0,
            state,
            execution_id: persisted.2,
            result_code,
            updated_at: persisted.8,
        })
    }

    pub(crate) fn fail_pre_execution_operation(
        &mut self,
        operation_handle: &str,
        result_code: &str,
    ) -> Result<DurableOperationStatus> {
        validate_public_operation_handle(operation_handle)?;
        validate_result_code(result_code)?;
        self.connection.execute(
            "UPDATE harness_operations
             SET operation_state = 'failed', operation_result_code = ?2,
                 gongbu_request_json = NULL, next_operation_attempt_at = NULL,
                 operation_updated_at = CURRENT_TIMESTAMP
             WHERE operation_handle = ?1
               AND gongbu_execution_id IS NULL
               AND operation_state IS NULL",
            params![operation_handle, result_code],
        )?;
        let status = self.durable_operation_status(operation_handle)?;
        if !status.terminal() {
            bail!("pre-execution operation could not be made terminal");
        }
        Ok(status)
    }

    pub(crate) fn promote_accepted_operations(&mut self) -> Result<usize> {
        Ok(self.connection.execute(
            "UPDATE harness_operations
             SET operation_state = 'queued', operation_updated_at = CURRENT_TIMESTAMP,
                 next_operation_attempt_at = CURRENT_TIMESTAMP
             WHERE operation_state = 'accepted'",
            [],
        )?)
    }

    pub(crate) fn claim_due_operation(&mut self) -> Result<Option<ClaimedDurableOperation>> {
        let now = timestamp(Utc::now());
        let lease_expires_at =
            timestamp(Utc::now() + chrono::Duration::seconds(WORKER_LEASE_SECONDS));
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let candidate = transaction
            .query_row(
                "SELECT operation_handle, operation_key, gongbu_request_json,
                        gongbu_execution_id, operation_state, dispatch_attempts,
                        observation_failures, reconciliation_attempts,
                        operation_deadline_at
                 FROM harness_operations
                 WHERE operation_state IN ('queued','dispatching','reconciling')
                   AND (next_operation_attempt_at IS NULL OR next_operation_attempt_at <= ?1)
                   AND (worker_lease_expires_at IS NULL OR worker_lease_expires_at <= ?1)
                 ORDER BY COALESCE(next_operation_attempt_at, created_at), created_at
                 LIMIT 1",
                [&now],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, u32>(5)?,
                        row.get::<_, u32>(6)?,
                        row.get::<_, u32>(7)?,
                        row.get::<_, Option<String>>(8)?,
                    ))
                },
            )
            .optional()?;
        let Some(candidate) = candidate else {
            transaction.commit()?;
            return Ok(None);
        };
        let operation_key = candidate
            .1
            .ok_or_else(|| anyhow!("durable operation is missing private operation identity"))?;
        let request = candidate
            .2
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?;
        if candidate.3.is_none() && request.is_none() {
            bail!("durable operation is missing its replay request");
        }
        let lease_id = Uuid::new_v4().simple().to_string();
        let claimed_state = if candidate.3.is_some() {
            "reconciling"
        } else {
            "dispatching"
        };
        let changed = transaction.execute(
            "UPDATE harness_operations
             SET operation_state = ?2, worker_lease_id = ?3,
                 worker_lease_expires_at = ?4, operation_updated_at = ?5
             WHERE operation_handle = ?1
               AND (worker_lease_expires_at IS NULL OR worker_lease_expires_at <= ?5)",
            params![candidate.0, claimed_state, lease_id, lease_expires_at, now],
        )?;
        if changed != 1 {
            transaction.commit()?;
            return Ok(None);
        }
        transaction.commit()?;
        Ok(Some(ClaimedDurableOperation {
            lease_id,
            operation_key,
            operation_handle: candidate.0,
            request,
            execution_id: candidate.3,
            dispatch_attempts: candidate.5,
            observation_failures: candidate.6,
            reconciliation_attempts: candidate.7,
            deadline_expired: candidate
                .8
                .as_deref()
                .is_some_and(|deadline| deadline <= now.as_str()),
        }))
    }

    pub(crate) fn retry_durable_operation(
        &mut self,
        operation: &ClaimedDurableOperation,
        delay: Duration,
        result_code: &str,
    ) -> Result<()> {
        validate_result_code(result_code)?;
        let next = timestamp(
            Utc::now()
                + chrono::Duration::from_std(delay)
                    .map_err(|_| anyhow!("durable operation retry delay is invalid"))?,
        );
        let dispatch = operation.execution_id.is_none();
        let changed = self.connection.execute(
            "UPDATE harness_operations
             SET operation_state = CASE WHEN gongbu_execution_id IS NULL THEN 'queued' ELSE 'reconciling' END,
                 dispatch_attempts = dispatch_attempts + ?3,
                 observation_failures = observation_failures + ?4,
                 operation_result_code = ?5,
                 next_operation_attempt_at = ?6,
                 worker_lease_id = NULL, worker_lease_expires_at = NULL,
                 operation_updated_at = CURRENT_TIMESTAMP
             WHERE operation_handle = ?1 AND worker_lease_id = ?2
               AND operation_state NOT IN ('succeeded','failed')",
            params![
                operation.operation_handle,
                operation.lease_id,
                i64::from(dispatch),
                i64::from(!dispatch),
                result_code,
                next,
            ],
        )?;
        ensure_lease_update(changed)
    }

    pub(crate) fn record_durable_lifecycle(
        &mut self,
        operation: &ClaimedDurableOperation,
        lifecycle: &GongbuLifecycle,
        next_delay: Duration,
        reconciliation_observation: bool,
    ) -> Result<()> {
        validate_gongbu_status(&lifecycle.status)?;
        if lifecycle.operation_key != operation.operation_key {
            bail!("Gongbu lifecycle conflicts with private operation identity");
        }
        let next = timestamp(
            Utc::now()
                + chrono::Duration::from_std(next_delay)
                    .map_err(|_| anyhow!("durable operation poll delay is invalid"))?,
        );
        let (state, result_code, terminal) = public_terminal_projection(lifecycle);
        let changed = self.connection.execute(
            "UPDATE harness_operations
             SET gongbu_execution_id = COALESCE(gongbu_execution_id, ?3),
                 gongbu_status = ?4, gongbu_outcome = ?5,
                 gongbu_result_recorded_at = CURRENT_TIMESTAMP,
                 gongbu_request_json = CASE WHEN ?3 IS NULL THEN gongbu_request_json ELSE NULL END,
                 operation_state = ?6, operation_result_code = ?7,
                 observation_failures = 0,
                 reconciliation_attempts = reconciliation_attempts + ?8,
                 next_operation_attempt_at = CASE WHEN ?9 THEN NULL ELSE ?10 END,
                 worker_lease_id = NULL, worker_lease_expires_at = NULL,
                 operation_updated_at = CURRENT_TIMESTAMP
             WHERE operation_handle = ?1 AND worker_lease_id = ?2
               AND operation_state NOT IN ('succeeded','failed')
               AND operation_key = ?11
               AND (gongbu_execution_id IS NULL OR gongbu_execution_id = ?3)",
            params![
                operation.operation_handle,
                operation.lease_id,
                lifecycle.execution_id,
                lifecycle.status,
                lifecycle.outcome,
                state,
                result_code,
                i64::from(reconciliation_observation),
                terminal,
                next,
                operation.operation_key,
            ],
        )?;
        ensure_lease_update(changed)
    }

    pub(crate) fn fail_durable_operation(
        &mut self,
        operation: &ClaimedDurableOperation,
        result_code: &str,
    ) -> Result<()> {
        validate_result_code(result_code)?;
        let changed = self.connection.execute(
            "UPDATE harness_operations
             SET operation_state = 'failed', operation_result_code = ?3,
                 gongbu_request_json = NULL,
                 next_operation_attempt_at = NULL,
                 worker_lease_id = NULL, worker_lease_expires_at = NULL,
                 operation_updated_at = CURRENT_TIMESTAMP
             WHERE operation_handle = ?1 AND worker_lease_id = ?2
               AND operation_state NOT IN ('succeeded','failed')",
            params![operation.operation_handle, operation.lease_id, result_code],
        )?;
        ensure_lease_update(changed)
    }

    pub(crate) fn fail_durable_lifecycle(
        &mut self,
        operation: &ClaimedDurableOperation,
        lifecycle: &GongbuLifecycle,
        result_code: &str,
    ) -> Result<()> {
        validate_gongbu_status(&lifecycle.status)?;
        validate_result_code(result_code)?;
        if lifecycle.operation_key != operation.operation_key {
            bail!("Gongbu lifecycle conflicts with private operation identity");
        }
        let changed = self.connection.execute(
            "UPDATE harness_operations
             SET gongbu_execution_id = COALESCE(gongbu_execution_id, ?3),
                 gongbu_status = ?4, gongbu_outcome = ?5,
                 gongbu_result_recorded_at = CURRENT_TIMESTAMP,
                 gongbu_request_json = NULL,
                 operation_state = 'failed', operation_result_code = ?6,
                 next_operation_attempt_at = NULL,
                 worker_lease_id = NULL, worker_lease_expires_at = NULL,
                 operation_updated_at = CURRENT_TIMESTAMP
             WHERE operation_handle = ?1 AND worker_lease_id = ?2
               AND operation_state NOT IN ('succeeded','failed')
               AND operation_key = ?7
               AND (gongbu_execution_id IS NULL OR gongbu_execution_id = ?3)",
            params![
                operation.operation_handle,
                operation.lease_id,
                lifecycle.execution_id,
                lifecycle.status,
                lifecycle.outcome,
                result_code,
                operation.operation_key,
            ],
        )?;
        ensure_lease_update(changed)
    }

    pub(crate) fn continuation_for_execution(
        &self,
        execution_id: &str,
    ) -> Result<Option<GongbuContinuation>> {
        validate_identifier("execution_id", execution_id, 255)?;
        let continuation = self
            .connection
            .query_row(
                "SELECT operation_key, operation_handle, gongbu_execution_id
                 FROM harness_operations WHERE gongbu_execution_id = ?1",
                [execution_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?;
        continuation
            .map(|(operation_key, operation_handle, execution_id)| {
                Ok(GongbuContinuation {
                    operation_key: operation_key.ok_or_else(|| {
                        anyhow!("Gongbu execution is missing private operation identity")
                    })?,
                    operation_handle,
                    execution_id,
                })
            })
            .transpose()
    }

    pub(crate) fn record_gongbu_lifecycle(
        &mut self,
        operation_handle: &str,
        lifecycle: &GongbuLifecycle,
    ) -> Result<()> {
        validate_gongbu_status(&lifecycle.status)?;
        validate_identifier("execution_id", &lifecycle.execution_id, 255)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = transaction
            .query_row(
                "SELECT operation_key, gongbu_execution_id
                 FROM harness_operations WHERE operation_handle = ?1",
                [operation_handle],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("Gongbu execution has no matching normalized operation"))?;
        if existing.0.as_deref() != Some(lifecycle.operation_key.as_str())
            || existing
                .1
                .as_deref()
                .is_some_and(|execution_id| execution_id != lifecycle.execution_id)
        {
            bail!("Gongbu execution identity conflicts with its normalized operation");
        }
        let (operation_state, operation_result_code, terminal) =
            public_terminal_projection(lifecycle);
        let changed = transaction.execute(
            "UPDATE harness_operations
             SET gongbu_execution_id = COALESCE(gongbu_execution_id, ?2),
                 gongbu_status = CASE
                     WHEN operation_state IN ('succeeded','failed')
                          AND ?3 NOT IN ('succeeded','released','failed')
                         THEN gongbu_status
                     ELSE ?3
                 END,
                 gongbu_outcome = CASE
                     WHEN operation_state IN ('succeeded','failed')
                          AND ?3 NOT IN ('succeeded','released','failed')
                         THEN gongbu_outcome
                     ELSE ?4
                 END,
                 gongbu_result_recorded_at = CURRENT_TIMESTAMP,
                 gongbu_request_json = NULL,
                 operation_state = CASE
                     WHEN operation_state IN ('succeeded','failed') THEN operation_state
                     ELSE ?6
                 END,
                 operation_result_code = CASE
                     WHEN operation_state IN ('succeeded','failed') THEN operation_result_code
                     ELSE ?7
                 END,
                 next_operation_attempt_at = CASE WHEN ?8 THEN NULL ELSE next_operation_attempt_at END,
                 worker_lease_id = CASE WHEN ?8 THEN NULL ELSE worker_lease_id END,
                 worker_lease_expires_at = CASE WHEN ?8 THEN NULL ELSE worker_lease_expires_at END,
                 operation_updated_at = CURRENT_TIMESTAMP
             WHERE operation_handle = ?1
               AND operation_key = ?5
               AND (gongbu_execution_id IS NULL OR gongbu_execution_id = ?2)",
            params![
                operation_handle,
                lifecycle.execution_id,
                lifecycle.status,
                lifecycle.outcome,
                lifecycle.operation_key,
                operation_state,
                operation_result_code,
                terminal,
            ],
        )?;
        if changed != 1 {
            bail!("Gongbu execution identity conflicts with its normalized operation");
        }
        transaction.commit()?;
        Ok(())
    }

    fn remove_expired_authorization_identifiers(&mut self) -> Result<()> {
        let now = Utc::now();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let rows = {
            let mut statement = transaction.prepare(
                "SELECT operation_handle, authorization_expires_at, result_json
                 FROM harness_operations
                 WHERE auth_token_id IS NOT NULL
                   AND authorization_expires_at IS NOT NULL
                   AND gongbu_create_started_at IS NULL",
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
            if DateTime::parse_from_rfc3339(&expires_at)
                .is_ok_and(|expires_at| expires_at.with_timezone(&Utc) > now)
            {
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
            transaction.execute(
                "UPDATE harness_operations
                 SET auth_token_id = NULL, result_json = ?2
                 WHERE operation_handle = ?1
                   AND gongbu_create_started_at IS NULL",
                params![handle, result_json],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }
}

pub(crate) fn validate_gongbu_request_size(arguments: &Value) -> Result<()> {
    if !arguments.is_object() {
        bail!("Gongbu execution arguments must be an object");
    }
    if request_exceeds_durable_limit(arguments)? {
        bail!("Gongbu execution request exceeds the durable adapter limit");
    }
    Ok(())
}

pub(crate) fn validate_durable_request_size(arguments: &Value) -> Result<()> {
    if !arguments.is_object() {
        bail!("Durable operation arguments must be an object");
    }
    if request_exceeds_durable_limit(arguments)? {
        bail!("Durable operation request exceeds the adapter limit");
    }
    Ok(())
}

fn request_exceeds_durable_limit(arguments: &Value) -> Result<bool> {
    struct BoundedJsonCounter {
        bytes: usize,
        exceeded: bool,
    }

    impl io::Write for BoundedJsonCounter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            let Some(total) = self.bytes.checked_add(buffer.len()) else {
                self.exceeded = true;
                return Err(io::Error::other("durable request size limit exceeded"));
            };
            if total > MAX_REQUEST_BYTES {
                self.exceeded = true;
                return Err(io::Error::other("durable request size limit exceeded"));
            }
            self.bytes = total;
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    // Compact JSON has the same byte count regardless of object-key ordering,
    // so a bounded streaming counter avoids cloning or buffering an oversized
    // request solely to measure its canonical durable representation.
    let mut counter = BoundedJsonCounter {
        bytes: 0,
        exceeded: false,
    };
    match serde_json::to_writer(&mut counter, arguments) {
        Ok(()) => Ok(false),
        Err(_) if counter.exceeded => Ok(true),
        Err(error) => Err(error.into()),
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
             tool_name TEXT NOT NULL CHECK(length(tool_name) BETWEEN 1 AND 128),
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
             gongbu_request_hash TEXT CHECK(gongbu_request_hash IS NULL OR length(gongbu_request_hash) = 71),
             gongbu_request_json TEXT CHECK(gongbu_request_json IS NULL OR (json_valid(gongbu_request_json) AND length(gongbu_request_json) <= 1048576)),
             gongbu_execution_id TEXT UNIQUE CHECK(gongbu_execution_id IS NULL OR length(gongbu_execution_id) BETWEEN 1 AND 255),
             gongbu_status TEXT CHECK(gongbu_status IS NULL OR gongbu_status IN ('pending','preflighting','claimed','executing','settling','succeeded','released','failed','reconciliation_required')),
             gongbu_outcome TEXT,
             gongbu_create_started_at TEXT,
             gongbu_result_recorded_at TEXT,
             operation_state TEXT CHECK(operation_state IS NULL OR operation_state IN ('accepted','queued','dispatching','reconciling','succeeded','failed')),
             operation_result_code TEXT CHECK(operation_result_code IS NULL OR length(operation_result_code) BETWEEN 1 AND 128),
             dispatch_attempts INTEGER NOT NULL DEFAULT 0 CHECK(dispatch_attempts >= 0),
             observation_failures INTEGER NOT NULL DEFAULT 0 CHECK(observation_failures >= 0),
             reconciliation_attempts INTEGER NOT NULL DEFAULT 0 CHECK(reconciliation_attempts >= 0),
             operation_deadline_at TEXT,
             next_operation_attempt_at TEXT,
             worker_lease_id TEXT CHECK(worker_lease_id IS NULL OR length(worker_lease_id) BETWEEN 1 AND 64),
             worker_lease_expires_at TEXT,
             operation_updated_at TEXT,
             created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
             PRIMARY KEY(platform, installation_id, harness_call_id),
             FOREIGN KEY(installation_id) REFERENCES installation_identity(installation_id)
         );
         CREATE UNIQUE INDEX harness_operation_auth_token
             ON harness_operations(auth_token_id) WHERE auth_token_id IS NOT NULL;
         CREATE INDEX harness_operation_due
             ON harness_operations(operation_state, next_operation_attempt_at, worker_lease_expires_at);
         PRAGMA application_id = {APPLICATION_ID};
         PRAGMA user_version = {SCHEMA_VERSION};
         COMMIT;"
    ))?;
    Ok(())
}

fn validate_gongbu_status(status: &str) -> Result<()> {
    if matches!(
        status,
        "pending"
            | "preflighting"
            | "claimed"
            | "executing"
            | "settling"
            | "succeeded"
            | "released"
            | "failed"
            | "reconciliation_required"
    ) {
        Ok(())
    } else {
        bail!("Gongbu returned an unsupported execution status")
    }
}

fn validate_public_operation_handle(handle: &str) -> Result<()> {
    if !handle.starts_with("hubu:public-operation:v1:") {
        bail!("public operation handle is invalid");
    }
    validate_identifier("operation_handle", handle, 160)
}

fn validate_result_code(code: &str) -> Result<()> {
    if code.is_empty()
        || code.len() > 128
        || !code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        bail!("durable operation result code is invalid");
    }
    Ok(())
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn public_terminal_projection(lifecycle: &GongbuLifecycle) -> (&'static str, &'static str, bool) {
    match lifecycle.status.as_str() {
        "succeeded" => ("succeeded", "execution_succeeded", true),
        "failed" => ("failed", "execution_failed", true),
        "released" => ("failed", "authorization_released", true),
        "reconciliation_required" => ("reconciling", "provider_outcome_ambiguous", false),
        _ => ("reconciling", "execution_in_progress", false),
    }
}

fn pre_execution_projection(
    tool_name: &str,
    decision: Option<&str>,
    has_authorization: bool,
) -> Result<(String, Option<String>)> {
    let projection = match (tool_name, decision, has_authorization) {
        ("hubu_authorize_spend" | "hubu_submit_governed_execution", Some("allow"), true) => {
            ("authorized", None)
        }
        ("hubu_authorize_spend" | "hubu_submit_governed_execution", Some("allow"), false) => {
            ("failed", Some("authorization_continuation_unavailable"))
        }
        ("hubu_authorize_spend" | "hubu_submit_governed_execution", Some("deny"), _) => {
            ("failed", Some("authorization_denied"))
        }
        ("hubu_submit_spend", Some("allow"), _) => ("succeeded", Some("spend_succeeded")),
        ("hubu_submit_spend", Some("deny"), _) => ("failed", Some("spend_denied")),
        (
            "hubu_authorize_spend" | "hubu_submit_spend" | "hubu_submit_governed_execution",
            Some("needs_approval"),
            _,
        ) => ("approval_required", Some("human_approval_required")),
        (
            "hubu_authorize_spend" | "hubu_submit_spend" | "hubu_submit_governed_execution",
            None,
            _,
        ) => ("awaiting_hubu_result", None),
        _ => bail!("normalized operation has an unsupported pre-execution state"),
    };
    Ok((projection.0.to_owned(), projection.1.map(str::to_owned)))
}

fn authorization_is_live(auth_token_id: Option<&str>, expires_at: Option<&str>) -> bool {
    if auth_token_id.is_none() {
        return false;
    }
    let Some(expires_at) = expires_at else {
        return true;
    };
    DateTime::parse_from_rfc3339(expires_at)
        .map(|expires_at| expires_at.with_timezone(&Utc) > Utc::now())
        .unwrap_or(false)
}

fn ensure_lease_update(changed: usize) -> Result<()> {
    if changed == 1 {
        Ok(())
    } else {
        bail!("durable operation worker lease was lost")
    }
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
            "SELECT platform, installation_id, harness_call_id, request_hash, tool_name, operation_key,
                    operation_handle,
                    codex_call_id, claude_tool_use_id, hubu_invocation_id,
                    controlled_installation_id, task_id, decision, decision_id,
                    auth_token_id, approval_request_id, authorization_expires_at,
                    result_json, dispatch_started_at, result_recorded_at,
                    gongbu_request_hash, gongbu_request_json, gongbu_execution_id, gongbu_status,
                    gongbu_outcome, gongbu_create_started_at,
                    gongbu_result_recorded_at, operation_state, operation_result_code,
                    dispatch_attempts, observation_failures, reconciliation_attempts,
                    operation_deadline_at, next_operation_attempt_at,
                    worker_lease_id, worker_lease_expires_at,
                    operation_updated_at, created_at
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

    fn execution_arguments(token: &str) -> Value {
        json!({
            "schema_version": 2,
            "spend_auth_token_id": token,
            "input": {"prompt": "durable prompt", "image_count": 1},
            "input_schema_version": 1,
            "workload_type": "image_generation",
            "provider": "fixture",
            "adapter": "fixture",
            "model": "fixture-v1"
        })
    }

    fn authorize_for_execution(
        registry: &mut OperationRegistry,
        call_id: &str,
        token: &str,
    ) -> OperationResolution {
        let operation = registry
            .resolve_or_allocate(
                &codex(call_id),
                "hubu_authorize_spend",
                &json!({"amount": 1}),
            )
            .unwrap();
        registry
            .record_authorization_result(
                &operation.operation_handle,
                &json!({
                    "decision": "allow",
                    "auth_token_id": token,
                    "authorization_expires_at": "2099-01-01T00:00:00Z",
                    "operation_handle": operation.operation_handle
                }),
            )
            .unwrap();
        operation
    }

    fn make_due(registry: &OperationRegistry, handle: &str) {
        registry
            .connection
            .execute(
                "UPDATE harness_operations
                 SET next_operation_attempt_at = '2000-01-01T00:00:00.000Z',
                     worker_lease_expires_at = NULL, worker_lease_id = NULL
                 WHERE operation_handle = ?1",
                [handle],
            )
            .unwrap();
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
    fn pre_execution_status_uses_tool_and_decision_instead_of_assuming_authorized() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let pending = registry
            .resolve_or_allocate(
                &codex("status-pending"),
                "hubu_authorize_spend",
                &json!({"amount": 1}),
            )
            .unwrap();
        assert_eq!(
            registry
                .durable_operation_status(&pending.operation_handle)
                .unwrap()
                .state,
            "awaiting_hubu_result"
        );

        let cases = [
            (
                "status-denied",
                "hubu_authorize_spend",
                json!({"decision":"deny"}),
                "failed",
                Some("authorization_denied"),
            ),
            (
                "status-approval",
                "hubu_authorize_spend",
                json!({"decision":"needs_approval"}),
                "approval_required",
                Some("human_approval_required"),
            ),
            (
                "status-missing-token",
                "hubu_authorize_spend",
                json!({"decision":"allow"}),
                "failed",
                Some("authorization_continuation_unavailable"),
            ),
            (
                "status-submitted",
                "hubu_submit_spend",
                json!({"decision":"allow","auth_token_id":"unused-submit-token"}),
                "succeeded",
                Some("spend_succeeded"),
            ),
        ];
        for (call_id, tool_name, result, expected_state, expected_code) in cases {
            let operation = registry
                .resolve_or_allocate(&codex(call_id), tool_name, &json!({"amount": 1}))
                .unwrap();
            registry
                .record_authorization_result(&operation.operation_handle, &result)
                .unwrap();
            let status = registry
                .durable_operation_status(&operation.operation_handle)
                .unwrap();
            assert_eq!(status.state, expected_state);
            assert_eq!(status.result_code.as_deref(), expected_code);
        }

        let submitted = registry
            .resolve_or_allocate(
                &codex("status-submit-no-continuation"),
                "hubu_submit_spend",
                &json!({"amount": 1}),
            )
            .unwrap();
        registry
            .record_authorization_result(
                &submitted.operation_handle,
                &json!({"decision":"allow","auth_token_id":"submit-is-not-continuation"}),
            )
            .unwrap();
        let error = registry
            .resolve_gongbu_continuation(
                "submit-is-not-continuation",
                &execution_arguments("submit-is-not-continuation"),
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("does not belong to an execution authorization"));

        let expired = registry
            .resolve_or_allocate(
                &codex("status-expired-authorization"),
                "hubu_authorize_spend",
                &json!({"amount": 1}),
            )
            .unwrap();
        registry
            .record_authorization_result(
                &expired.operation_handle,
                &json!({
                    "decision":"allow",
                    "auth_token_id":"expired-status-token",
                    "authorization_expires_at":"2000-01-01T00:00:00Z"
                }),
            )
            .unwrap();
        let status = registry
            .durable_operation_status(&expired.operation_handle)
            .unwrap();
        assert_eq!(status.state, "failed");
        assert_eq!(
            status.result_code.as_deref(),
            Some("authorization_continuation_unavailable")
        );

        let malformed = registry
            .resolve_or_allocate(
                &codex("status-malformed-authorization-expiry"),
                "hubu_authorize_spend",
                &json!({"amount": 1}),
            )
            .unwrap();
        registry
            .record_authorization_result(
                &malformed.operation_handle,
                &json!({
                    "decision":"allow",
                    "auth_token_id":"malformed-expiry-token",
                    "authorization_expires_at":"not-a-timestamp"
                }),
            )
            .unwrap();
        let status = registry
            .durable_operation_status(&malformed.operation_handle)
            .unwrap();
        assert_eq!(status.state, "failed");
        assert_eq!(
            status.result_code.as_deref(),
            Some("authorization_continuation_unavailable")
        );
        let error = registry
            .resolve_gongbu_continuation(
                "malformed-expiry-token",
                &execution_arguments("malformed-expiry-token"),
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("unknown or expired"));
    }

    #[test]
    fn durable_lifecycle_persists_accept_queue_dispatch_observe_and_success() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let operation = authorize_for_execution(&mut registry, "durable-success", "token-success");
        assert_eq!(
            registry
                .durable_operation_status(&operation.operation_handle)
                .unwrap()
                .state,
            "authorized"
        );
        let arguments = execution_arguments("token-success");
        let continuation = registry
            .resolve_gongbu_continuation("token-success", &arguments)
            .unwrap();
        assert_eq!(
            registry
                .durable_operation_status(&operation.operation_handle)
                .unwrap()
                .state,
            "accepted"
        );
        let stored_request: String = registry
            .connection
            .query_row(
                "SELECT gongbu_request_json FROM harness_operations WHERE operation_handle = ?1",
                [&operation.operation_handle],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&stored_request).unwrap(),
            arguments
        );

        assert_eq!(registry.promote_accepted_operations().unwrap(), 1);
        assert_eq!(
            registry
                .durable_operation_status(&operation.operation_handle)
                .unwrap()
                .state,
            "queued"
        );
        let claimed = registry.claim_due_operation().unwrap().unwrap();
        assert_eq!(
            registry
                .durable_operation_status(&operation.operation_handle)
                .unwrap()
                .state,
            "dispatching"
        );
        registry
            .record_durable_lifecycle(
                &claimed,
                &GongbuLifecycle {
                    execution_id: "execution-success".into(),
                    operation_key: continuation.operation_key.clone(),
                    status: "pending".into(),
                    outcome: None,
                },
                Duration::ZERO,
                false,
            )
            .unwrap();
        let observing = registry
            .durable_operation_status(&operation.operation_handle)
            .unwrap();
        assert_eq!(observing.state, "reconciling");
        assert_eq!(observing.execution_id.as_deref(), Some("execution-success"));
        let request_after_identity: Option<String> = registry
            .connection
            .query_row(
                "SELECT gongbu_request_json FROM harness_operations WHERE operation_handle = ?1",
                [&operation.operation_handle],
                |row| row.get(0),
            )
            .unwrap();
        assert!(request_after_identity.is_none());

        make_due(&registry, &operation.operation_handle);
        let claimed = registry.claim_due_operation().unwrap().unwrap();
        registry
            .record_durable_lifecycle(
                &claimed,
                &GongbuLifecycle {
                    execution_id: "execution-success".into(),
                    operation_key: continuation.operation_key,
                    status: "succeeded".into(),
                    outcome: Some("succeeded".into()),
                },
                Duration::ZERO,
                false,
            )
            .unwrap();
        let terminal = registry
            .durable_operation_status(&operation.operation_handle)
            .unwrap();
        assert_eq!(terminal.state, "succeeded");
        assert_eq!(terminal.result_code.as_deref(), Some("execution_succeeded"));
        assert!(terminal.terminal());
        assert!(registry.claim_due_operation().unwrap().is_none());
    }

    #[test]
    fn terminal_direct_observation_wins_over_stale_worker_lease() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let operation = authorize_for_execution(&mut registry, "terminal-race", "token-race");
        let continuation = registry
            .resolve_gongbu_continuation("token-race", &execution_arguments("token-race"))
            .unwrap();
        registry.promote_accepted_operations().unwrap();
        let dispatch = registry.claim_due_operation().unwrap().unwrap();
        let pending = GongbuLifecycle {
            execution_id: "execution-race".into(),
            operation_key: continuation.operation_key.clone(),
            status: "pending".into(),
            outcome: None,
        };
        registry
            .record_durable_lifecycle(&dispatch, &pending, Duration::ZERO, false)
            .unwrap();

        make_due(&registry, &operation.operation_handle);
        let stale_observer = registry.claim_due_operation().unwrap().unwrap();
        registry
            .record_gongbu_lifecycle(
                &operation.operation_handle,
                &GongbuLifecycle {
                    execution_id: "execution-race".into(),
                    operation_key: continuation.operation_key,
                    status: "succeeded".into(),
                    outcome: Some("succeeded".into()),
                },
            )
            .unwrap();

        assert!(registry
            .record_durable_lifecycle(&stale_observer, &pending, Duration::ZERO, false)
            .is_err());
        let status = registry
            .durable_operation_status(&operation.operation_handle)
            .unwrap();
        assert_eq!(status.state, "succeeded");
        assert_eq!(status.result_code.as_deref(), Some("execution_succeeded"));
        let backend_status: String = registry
            .connection
            .query_row(
                "SELECT gongbu_status FROM harness_operations WHERE operation_handle = ?1",
                [&operation.operation_handle],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(backend_status, "succeeded");
    }

    #[test]
    fn ambiguous_dispatch_replays_exact_request_after_lease_expiry_and_restart() {
        let root = tempdir().unwrap();
        let path = root.path().join("operations.sqlite3");
        let arguments = execution_arguments("token-restart");
        let operation = {
            let mut registry = OperationRegistry::open(&path).unwrap();
            let operation =
                authorize_for_execution(&mut registry, "durable-restart", "token-restart");
            registry
                .resolve_gongbu_continuation("token-restart", &arguments)
                .unwrap();
            registry.promote_accepted_operations().unwrap();
            let claimed = registry.claim_due_operation().unwrap().unwrap();
            assert_eq!(claimed.request, Some(arguments.clone()));
            registry
                .connection
                .execute(
                    "UPDATE harness_operations SET worker_lease_expires_at = '2000-01-01T00:00:00.000Z'
                     WHERE operation_handle = ?1",
                    [&operation.operation_handle],
                )
                .unwrap();
            operation
        };
        let mut restarted = OperationRegistry::open(&path).unwrap();
        let replay = restarted.claim_due_operation().unwrap().unwrap();
        assert_eq!(replay.request, Some(arguments));
        assert_eq!(replay.operation_key, operation.operation_key.unwrap());
        assert!(replay.execution_id.is_none());
    }

    #[test]
    fn retry_exhaustion_is_terminal_and_replacement_is_not_inferred() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let operation =
            authorize_for_execution(&mut registry, "durable-exhausted", "token-exhausted");
        registry
            .resolve_gongbu_continuation("token-exhausted", &execution_arguments("token-exhausted"))
            .unwrap();
        registry.promote_accepted_operations().unwrap();
        let claimed = registry.claim_due_operation().unwrap().unwrap();
        registry
            .retry_durable_operation(&claimed, Duration::ZERO, "dispatch_retry_pending")
            .unwrap();
        make_due(&registry, &operation.operation_handle);
        let claimed = registry.claim_due_operation().unwrap().unwrap();
        registry
            .fail_durable_operation(&claimed, "dispatch_retry_exhausted")
            .unwrap();
        let terminal = registry
            .durable_operation_status(&operation.operation_handle)
            .unwrap();
        assert_eq!(terminal.state, "failed");
        assert_eq!(
            terminal.result_code.as_deref(),
            Some("dispatch_retry_exhausted")
        );
        assert!(terminal.terminal());
    }

    #[test]
    fn durable_deadline_bounds_otherwise_nonterminal_execution_observation() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let operation = authorize_for_execution(&mut registry, "deadline", "token-deadline");
        registry
            .resolve_gongbu_continuation("token-deadline", &execution_arguments("token-deadline"))
            .unwrap();
        registry
            .connection
            .execute(
                "UPDATE harness_operations
                 SET operation_deadline_at = '2000-01-01T00:00:00.000Z'
                 WHERE operation_handle = ?1",
                [&operation.operation_handle],
            )
            .unwrap();
        registry.promote_accepted_operations().unwrap();
        let claimed = registry.claim_due_operation().unwrap().unwrap();
        assert!(claimed.deadline_expired);
        registry
            .fail_durable_operation(&claimed, "operation_deadline_exhausted")
            .unwrap();
        let status = registry
            .durable_operation_status(&operation.operation_handle)
            .unwrap();
        assert_eq!(status.state, "failed");
        assert_eq!(
            status.result_code.as_deref(),
            Some("operation_deadline_exhausted")
        );
    }

    #[test]
    fn reconciliation_exhaustion_keeps_backend_ambiguity_but_terminates_adapter() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let operation =
            authorize_for_execution(&mut registry, "reconciliation", "token-reconciliation");
        let continuation = registry
            .resolve_gongbu_continuation(
                "token-reconciliation",
                &execution_arguments("token-reconciliation"),
            )
            .unwrap();
        registry.promote_accepted_operations().unwrap();
        let claimed = registry.claim_due_operation().unwrap().unwrap();
        let lifecycle = GongbuLifecycle {
            execution_id: "execution-reconciliation".into(),
            operation_key: continuation.operation_key,
            status: "reconciliation_required".into(),
            outcome: Some("ambiguous".into()),
        };
        registry
            .record_durable_lifecycle(&claimed, &lifecycle, Duration::ZERO, true)
            .unwrap();
        make_due(&registry, &operation.operation_handle);
        let claimed = registry.claim_due_operation().unwrap().unwrap();
        registry
            .fail_durable_lifecycle(&claimed, &lifecycle, "reconciliation_exhausted")
            .unwrap();
        let status = registry
            .durable_operation_status(&operation.operation_handle)
            .unwrap();
        assert_eq!(status.state, "failed");
        assert_eq!(
            status.result_code.as_deref(),
            Some("reconciliation_exhausted")
        );
        let persisted_backend_status: String = registry
            .connection
            .query_row(
                "SELECT gongbu_status FROM harness_operations WHERE operation_handle = ?1",
                [&operation.operation_handle],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(persisted_backend_status, "reconciliation_required");
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
    fn terminal_result_replays_with_private_key_retained_only_in_registry() {
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
        assert_eq!(recovered_terminal.operation_key, pending.operation_key);
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
                pending.operation_key,
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
        assert_eq!(recovered.operation_key, operation.operation_key);
        assert_eq!(recovered.recorded_result, Some(terminal));
    }

    #[test]
    fn dispatch_boundary_returns_terminal_result_that_won_the_race() {
        let mut first = OperationRegistry::open_in_memory().unwrap();
        let stale_resolution = first
            .resolve_or_allocate(
                &codex("dispatch-race"),
                "hubu_authorize_spend",
                &json!({"amount": 1}),
            )
            .unwrap();
        let terminal = json!({
            "decision":"deny",
            "decision_id":"terminal-decision",
            "operation_handle":stale_resolution.operation_handle
        });
        first
            .record_authorization_result(&stale_resolution.operation_handle, &terminal)
            .unwrap();

        assert_eq!(
            first
                .mark_dispatch_started(&stale_resolution.operation_handle)
                .unwrap(),
            Some(terminal)
        );
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
    fn expired_authorization_identifier_survives_ambiguous_gongbu_dispatch_restart() {
        let root = tempdir().unwrap();
        let path = root.path().join("operations.sqlite3");
        let arguments = json!({
            "schema_version": 2,
            "spend_auth_token_id": "dispatched-authorization",
            "input": {"prompt": "recover me"},
            "input_schema_version": 1,
            "workload_type": "image_generation",
            "provider": "fixture",
            "adapter": "fixture",
            "model": "fixture-v1"
        });
        let mut registry = OperationRegistry::open(&path).unwrap();
        let operation = registry
            .resolve_or_allocate(
                &codex("ambiguous-gongbu-dispatch"),
                "hubu_authorize_spend",
                &json!({"amount": 1}),
            )
            .unwrap();
        registry
            .record_authorization_result(
                &operation.operation_handle,
                &json!({
                    "decision":"allow",
                    "auth_token_id":"dispatched-authorization",
                    "authorization_expires_at":"2099-01-01T00:00:00Z",
                    "operation_handle":operation.operation_handle
                }),
            )
            .unwrap();
        let dispatched = registry
            .resolve_gongbu_continuation("dispatched-authorization", &arguments)
            .unwrap();
        assert!(dispatched.execution_id.is_none());
        registry
            .connection
            .execute(
                "UPDATE harness_operations
                 SET authorization_expires_at = '2020-01-01T00:00:00Z'
                 WHERE operation_handle = ?1",
                [&operation.operation_handle],
            )
            .unwrap();
        drop(registry);

        let mut restarted = OperationRegistry::open(&path).unwrap();
        let recovered = restarted
            .resolve_gongbu_continuation("dispatched-authorization", &arguments)
            .unwrap();
        assert_eq!(recovered.operation_handle, operation.operation_handle);
        assert_eq!(recovered.operation_key, operation.operation_key.unwrap());
        assert!(recovered.execution_id.is_none());
    }

    #[test]
    fn v1_registry_is_rejected_for_the_v4_only_fresh_profile_contract() {
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

        let error = OperationRegistry::open(&path).unwrap_err().to_string();
        assert!(error.contains("schema version is unsupported"));
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn v3_registry_is_rejected_without_mutation() {
        let root = tempdir().unwrap();
        let path = root.path().join("operations.sqlite3");
        drop(OperationRegistry::open(&path).unwrap());
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch("PRAGMA user_version = 3;")
            .unwrap();
        drop(connection);
        assert!(OperationRegistry::open(&path).is_err());
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            3
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
    fn v2_registry_is_rejected_without_advertising_unrecoverable_continuations() {
        let root = tempdir().unwrap();
        let path = root.path().join("operations-v2.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(&format!(
                "CREATE TABLE harness_operations (
                     operation_handle TEXT PRIMARY KEY,
                     operation_key TEXT,
                     auth_token_id TEXT,
                     result_json TEXT
                 );
                 INSERT INTO harness_operations(
                     operation_handle, operation_key, auth_token_id, result_json
                 ) VALUES(
                     'hubu:public-operation:v1:legacy', NULL,
                     'unrecoverable-token',
                     '{{\"decision\":\"allow\",\"auth_token_id\":\"unrecoverable-token\"}}'
                 );
                 PRAGMA application_id = {APPLICATION_ID};
                 PRAGMA user_version = 2;"
            ))
            .unwrap();
        drop(connection);

        let error = OperationRegistry::open(&path).unwrap_err().to_string();
        assert!(error.contains("schema version is unsupported"));

        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row("SELECT auth_token_id FROM harness_operations", [], |row| {
                    row.get::<_, String>(0)
                },)
                .unwrap(),
            "unrecoverable-token"
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
