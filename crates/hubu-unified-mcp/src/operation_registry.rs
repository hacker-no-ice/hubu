use std::{
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const CONTROLLED_INVOCATION_META_KEY: &str = "hubu.dev/platform-invocation";

const CODEX_CALL_ID_KEY: &str = "callId";
const CLAUDE_TOOL_USE_ID_KEY: &str = "claudecode/toolUseId";
const MAX_PLATFORM_BYTES: usize = 64;
const MAX_HARNESS_ID_BYTES: usize = 512;
const MAX_TASK_ID_BYTES: usize = 512;
const MAX_TOOL_NAME_BYTES: usize = 128;
const LEGACY_SCHEMA_VERSION: i64 = 4;
const PREVIOUS_SCHEMA_VERSION: i64 = 5;
const SCHEMA_VERSION: i64 = 6;
const APPLICATION_ID: i64 = 0x4855_424f;
const MAX_RESULT_BYTES: usize = 1024 * 1024;
const MAX_GOVERNED_RESULT_BYTES: usize = 16 * 1024 * 1024;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResumeOrigin {
    AuthorizeSpend,
    SubmitSpend,
    GovernedExecution,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalPaymentReceipt {
    Succeeded,
    Failed,
}

impl TerminalPaymentReceipt {
    fn operation_state(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    fn result_code(self) -> &'static str {
        match self {
            Self::Succeeded => "spend_succeeded",
            Self::Failed => "spend_failed",
        }
    }
}

impl ResumeOrigin {
    pub(crate) fn normalized_tool_name(self) -> &'static str {
        match self {
            Self::AuthorizeSpend => "hubu_authorize_spend",
            Self::SubmitSpend => "hubu_submit_spend",
            Self::GovernedExecution => crate::governed_execution::TOOL_NAME,
        }
    }

    pub(crate) fn hubu_tool_name(self) -> &'static str {
        match self {
            Self::AuthorizeSpend | Self::GovernedExecution => "hubu_authorize_spend",
            Self::SubmitSpend => "hubu_submit_spend",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResumeOperationPlan {
    pub(crate) origin: ResumeOrigin,
    pub(crate) operation: OperationResolution,
    pub(crate) hubu_arguments: Value,
}

impl ResumeOperationPlan {
    pub(crate) fn hubu_tool_name(&self) -> &'static str {
        self.origin.hubu_tool_name()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ResumePreparation {
    Dispatch(ResumeOperationPlan),
    Replay {
        origin: ResumeOrigin,
        authoritative_result: Value,
        status: DurableOperationStatus,
    },
    Status(DurableOperationStatus),
    IntentUnavailable(DurableOperationStatus),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResumeCompletion {
    pub(crate) authoritative_result: Value,
    pub(crate) status: DurableOperationStatus,
    pub(crate) wake_operation_worker: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ApprovalSyncTarget {
    pub(crate) operation_handle: String,
    pub(crate) approval_request_id: String,
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
    normalized_request_json: Option<String>,
    tool_name: String,
    operation_key: Option<String>,
    operation_handle: String,
    codex_call_id: Option<String>,
    claude_tool_use_id: Option<String>,
    hubu_invocation_id: Option<String>,
    task_id: Option<String>,
    decision: Option<String>,
    result_json: Option<String>,
    operation_state: Option<String>,
}

pub(crate) struct OperationRegistry {
    connection: Connection,
    installation_id: String,
    preallocated_operation_key_path: Option<PathBuf>,
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
    #[cfg(test)]
    pub(crate) fn open(path: &Path) -> Result<Self> {
        Self::open_with_preallocated_keys(path, None)
    }

    pub(crate) fn open_with_preallocated_keys(
        path: &Path,
        preallocated_operation_key_path: Option<&Path>,
    ) -> Result<Self> {
        if path == Path::new(":memory:") {
            bail!("unified MCP operation registry requires a persistent on-disk path; in-memory state is test-only");
        }
        if !path.is_absolute() {
            bail!("unified MCP operation registry path must be absolute");
        }
        if preallocated_operation_key_path.is_some_and(|path| !path.is_absolute()) {
            bail!("preallocated operation key store path must be absolute");
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
        Self::from_connection(
            connection,
            preallocated_operation_key_path.map(Path::to_path_buf),
        )
    }

    fn from_connection(
        mut connection: Connection,
        preallocated_operation_key_path: Option<PathBuf>,
    ) -> Result<Self> {
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
        } else if application_id == APPLICATION_ID && version == LEGACY_SCHEMA_VERSION {
            migrate_v4_to_v5(&mut connection)?;
            migrate_v5_to_v6(&mut connection)?;
        } else if application_id == APPLICATION_ID && version == PREVIOUS_SCHEMA_VERSION {
            migrate_v5_to_v6(&mut connection)?;
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
            preallocated_operation_key_path,
        };
        registry.remove_expired_authorization_identifiers()?;
        Ok(registry)
    }

    #[cfg(test)]
    pub(crate) fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?, None)
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
        validate_durable_request_size(arguments)?;
        let request_hash = canonical_request_hash(tool_name, arguments)?;
        let normalized_request_json = serde_json::to_string(&canonicalize(arguments))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = transaction
            .query_row(
                "SELECT request_hash, normalized_request_json, tool_name, operation_key, operation_handle, codex_call_id,
                        claude_tool_use_id, hubu_invocation_id, task_id, decision,
                        result_json, operation_state
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
                        normalized_request_json: row.get(1)?,
                        tool_name: row.get(2)?,
                        operation_key: row.get(3)?,
                        operation_handle: row.get(4)?,
                        codex_call_id: row.get(5)?,
                        claude_tool_use_id: row.get(6)?,
                        hubu_invocation_id: row.get(7)?,
                        task_id: row.get(8)?,
                        decision: row.get(9)?,
                        result_json: row.get(10)?,
                        operation_state: row.get(11)?,
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
            if existing
                .normalized_request_json
                .as_deref()
                .is_some_and(|persisted| persisted != normalized_request_json)
            {
                bail!("stored normalized operation intent conflicts with its canonical request hash; refusing backend access");
            }
            // A v4 in-flight or pending row may safely recover its canonical
            // intent only from exact harness redelivery. Terminal/direct and
            // already-bound governed rows intentionally discarded that intent;
            // replay must not repersist it after its lifecycle purpose ended.
            if existing.normalized_request_json.is_none()
                && !matches!(existing.decision.as_deref(), Some("allow" | "deny"))
                && !existing
                    .operation_state
                    .as_deref()
                    .is_some_and(is_durable_terminal_state)
            {
                let changed = transaction.execute(
                    "UPDATE harness_operations
                     SET normalized_request_json = ?2
                     WHERE operation_handle = ?1 AND normalized_request_json IS NULL",
                    params![existing.operation_handle, normalized_request_json],
                )?;
                if changed != 1 {
                    bail!("normalized operation intent could not be recovered");
                }
            }
            transaction.commit()?;
            // Once Hubu has durably returned needs_approval, the original
            // submission path becomes replay-only. Approval may advance that
            // immutable operation only through prepare_resume/complete_resume;
            // exact original-call redelivery must never obtain a continuation
            // and bypass the public-handle boundary.
            let recorded_result = existing
                .result_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .context("decode recorded normalized operation result")?;
            return Ok(OperationResolution {
                operation_key: existing.operation_key,
                operation_handle: existing.operation_handle,
                task_id: existing.task_id,
                recorded_result,
            });
        }

        let (operation_key, operation_key_record_id) =
            if let Some(path) = self.preallocated_operation_key_path.as_deref() {
                if identity.codex_call_id.is_none() {
                    bail!("preallocated operation keys require trusted Codex callId identity");
                }
                let record = preallocated_operation_key(
                    path,
                    tool_name,
                    arguments,
                    &self.installation_id,
                    identity,
                    &request_hash,
                )
                .map_err(|error| {
                    if error.to_string().contains("permissions are unsafe") {
                        error
                    } else {
                        anyhow!("preallocated operation key record is already bound or invalid")
                    }
                })?;
                (record.operation_key, Some(record.record_id))
            } else {
                (
                    format!(
                        "hubu:operation:v1:{}:{}",
                        identity.platform,
                        Uuid::new_v4().simple()
                    ),
                    None,
                )
            };
        let operation_handle = format!("hubu:public-operation:v1:{}", Uuid::new_v4().simple());
        transaction
            .execute(
                "INSERT INTO harness_operations (
                 platform, installation_id, harness_call_id, request_hash, normalized_request_json,
                 tool_name, operation_key, operation_key_record_id,
                 operation_handle,
                 codex_call_id, claude_tool_use_id, hubu_invocation_id,
                 controlled_installation_id, task_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    identity.platform,
                    self.installation_id,
                    identity.harness_call_id,
                    request_hash,
                    normalized_request_json,
                    tool_name,
                    operation_key,
                    operation_key_record_id,
                    operation_handle,
                    identity.codex_call_id,
                    identity.claude_tool_use_id,
                    identity.hubu_invocation_id,
                    identity.controlled_installation_id,
                    identity.task_id,
                ],
            )
            .map_err(|error| {
                if self.preallocated_operation_key_path.is_some() {
                    anyhow!("preallocated operation key record is already bound or invalid")
                } else {
                    error.into()
                }
            })?;
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
        let (operation_key, decision, result_json, operation_state) = transaction
            .query_row(
                "SELECT operation_key, decision, result_json, operation_state
                 FROM harness_operations WHERE operation_handle = ?1",
                [operation_handle],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("normalized operation cannot be dispatched"))?;
        if let Some(result_json) = result_json.as_deref() {
            let result = serde_json::from_str(result_json)
                .context("decode recorded normalized operation result")?;
            transaction.commit()?;
            return Ok(Some(result));
        }
        if decision.is_some()
            || operation_state
                .as_deref()
                .is_some_and(is_durable_terminal_state)
        {
            bail!("recorded normalized operation is missing replay state");
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
        let (
            private_operation_key,
            existing_decision,
            existing_result,
            existing_approval_request_id,
            existing_approval_status,
            tool_name,
        ) = transaction
            .query_row(
                "SELECT operation_key, decision, result_json, approval_request_id,
                        approval_status, tool_name
                 FROM harness_operations WHERE operation_handle = ?1",
                [operation_handle],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
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
        if existing_decision.is_some() {
            let existing_result = existing_result
                .as_deref()
                .ok_or_else(|| anyhow!("recorded normalized operation is missing replay state"))
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
        if let Some(approval_request_id) = approval_request_id.as_deref() {
            validate_identifier("approval_request_id", approval_request_id, 255)?;
        }
        if existing_approval_request_id
            .as_deref()
            .zip(approval_request_id.as_deref())
            .is_some_and(|(existing, incoming)| existing != incoming)
        {
            bail!("Hubu spend authorization result conflicts with its approval request");
        }
        let has_approval = approval_request_id.is_some()
            || existing_approval_request_id.is_some()
            || existing_approval_status.is_some();
        let candidate_approval_status = has_approval.then_some(match decision {
            "needs_approval" => "pending",
            "allow" => "approved",
            "deny" => "denied",
            _ => unreachable!("decision was validated above"),
        });
        let approval_status = monotonic_approval_status(
            existing_approval_status.as_deref(),
            candidate_approval_status,
        )?;
        let authorization_expires_at = optional_string(result, "authorization_expires_at")?;
        let clear_normalized_request = decision == "deny"
            || (decision == "allow" && tool_name != crate::governed_execution::TOOL_NAME);
        let changed = transaction.execute(
            "UPDATE harness_operations
             SET decision = ?2,
                 decision_id = ?3,
                 auth_token_id = ?4,
                 approval_request_id = COALESCE(?5, approval_request_id),
                 approval_status = COALESCE(?6, approval_status),
                 approval_synced_at = CASE WHEN ?6 IS NULL THEN approval_synced_at ELSE CURRENT_TIMESTAMP END,
                 authorization_expires_at = ?7,
                 result_json = ?8,
                 normalized_request_json = CASE WHEN ?9 THEN NULL ELSE normalized_request_json END,
                 result_recorded_at = CURRENT_TIMESTAMP
             WHERE operation_handle = ?1",
            params![
                operation_handle,
                decision,
                decision_id,
                auth_token_id,
                approval_request_id,
                approval_status,
                authorization_expires_at,
                result_json,
                clear_normalized_request,
            ],
        )?;
        if changed != 1 {
            bail!("normalized operation result has no matching operation");
        }
        transaction.commit()?;
        Ok(result.clone())
    }

    pub(crate) fn governed_result(&self, operation_handle: &str) -> Result<Option<Value>> {
        validate_public_operation_handle(operation_handle)?;
        let stored = self
            .connection
            .query_row(
                "SELECT operation_key, governed_result_json
                 FROM harness_operations
                 WHERE operation_handle = ?1 AND tool_name = ?2",
                params![operation_handle, crate::governed_execution::TOOL_NAME],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .optional()?;
        let Some((operation_key, result_json)) = stored else {
            return Ok(None);
        };
        if operation_key
            .as_deref()
            .zip(result_json.as_deref())
            .is_some_and(|(operation_key, result)| result.contains(operation_key))
        {
            bail!("recorded governed execution result contains private backend identity");
        }
        let result = result_json
            .as_deref()
            .map(|result| {
                serde_json::from_str(result).context("decode recorded governed execution result")
            })
            .transpose()?;
        if result
            .as_ref()
            .is_some_and(|result| !valid_terminal_governed_result(result, operation_handle))
        {
            bail!("recorded governed execution result is not a safe terminal projection");
        }
        Ok(result)
    }

    pub(crate) fn record_governed_result(
        &mut self,
        operation_handle: &str,
        result: &Value,
    ) -> Result<Value> {
        validate_public_operation_handle(operation_handle)?;
        if !valid_terminal_governed_result(result, operation_handle) {
            bail!("governed execution result is not a safe terminal projection");
        }
        let result_json = serde_json::to_string(result)?;
        if result_json.len() > MAX_GOVERNED_RESULT_BYTES {
            bail!("governed execution result is too large to persist safely");
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (operation_key, existing) = transaction
            .query_row(
                "SELECT operation_key, governed_result_json
                 FROM harness_operations
                 WHERE operation_handle = ?1 AND tool_name = ?2",
                params![operation_handle, crate::governed_execution::TOOL_NAME],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("governed execution result has no matching operation"))?;
        if operation_key
            .as_deref()
            .is_some_and(|operation_key| result_json.contains(operation_key))
        {
            bail!("governed execution result contains private backend identity");
        }
        if let Some(existing) = existing {
            let result = serde_json::from_str(&existing)
                .context("decode recorded governed execution result")?;
            transaction.commit()?;
            return Ok(result);
        }
        let changed = transaction.execute(
            "UPDATE harness_operations
             SET governed_result_json = ?2
             WHERE operation_handle = ?1 AND tool_name = ?3
                   AND governed_result_json IS NULL",
            params![
                operation_handle,
                result_json,
                crate::governed_execution::TOOL_NAME
            ],
        )?;
        if changed != 1 {
            bail!("governed execution result could not be persisted");
        }
        transaction.commit()?;
        Ok(result.clone())
    }

    pub(crate) fn approval_sync_target_for_handle(
        &self,
        operation_handle: &str,
    ) -> Result<Option<ApprovalSyncTarget>> {
        validate_public_operation_handle(operation_handle)?;
        self.connection
            .query_row(
                "SELECT operation_handle, approval_request_id
                 FROM harness_operations
                 WHERE operation_handle = ?1 AND approval_request_id IS NOT NULL",
                [operation_handle],
                |row| {
                    Ok(ApprovalSyncTarget {
                        operation_handle: row.get(0)?,
                        approval_request_id: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    #[cfg(test)]
    fn approval_sync_target_for_request(
        &self,
        approval_request_id: &str,
    ) -> Result<Option<ApprovalSyncTarget>> {
        validate_identifier("approval_request_id", approval_request_id, 255)?;
        self.connection
            .query_row(
                "SELECT operation_handle, approval_request_id
                 FROM harness_operations WHERE approval_request_id = ?1",
                [approval_request_id],
                |row| {
                    Ok(ApprovalSyncTarget {
                        operation_handle: row.get(0)?,
                        approval_request_id: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub(crate) fn synchronize_approval_status(
        &mut self,
        approval_request_id: &str,
        status: &str,
    ) -> Result<Option<DurableOperationStatus>> {
        validate_identifier("approval_request_id", approval_request_id, 255)?;
        validate_approval_status(status)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = transaction
            .query_row(
                "SELECT operation_handle, approval_status, decision
                 FROM harness_operations WHERE approval_request_id = ?1",
                [approval_request_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((operation_handle, existing_status, decision)) = existing else {
            transaction.commit()?;
            return Ok(None);
        };
        let decision_status = match decision.as_deref() {
            Some("needs_approval") => Some("pending"),
            Some("allow") => Some("approved"),
            Some("deny") => Some("denied"),
            None => None,
            Some(_) => bail!("normalized operation has an unsupported authorization decision"),
        };
        let established = monotonic_approval_status(existing_status.as_deref(), decision_status)?;
        let synchronized = monotonic_approval_status(established.as_deref(), Some(status))?
            .ok_or_else(|| anyhow!("approval status could not be synchronized"))?;
        let changed = transaction.execute(
            "UPDATE harness_operations
             SET approval_status = ?2,
                 approval_synced_at = CURRENT_TIMESTAMP,
                 normalized_request_json = CASE
                     WHEN ?2 = 'denied' THEN NULL
                     ELSE normalized_request_json
                 END
             WHERE operation_handle = ?1 AND approval_request_id = ?3",
            params![operation_handle, synchronized, approval_request_id],
        )?;
        if changed != 1 {
            bail!("approval status has no matching normalized operation");
        }
        transaction.commit()?;
        Ok(Some(self.durable_operation_status(&operation_handle)?))
    }

    pub(crate) fn prepare_resume(&mut self, operation_handle: &str) -> Result<ResumePreparation> {
        validate_public_operation_handle(operation_handle)?;
        let persisted = self
            .connection
            .query_row(
                "SELECT tool_name, operation_key, task_id, request_hash,
                        normalized_request_json, approval_status, decision, result_json
                 FROM harness_operations WHERE operation_handle = ?1",
                [operation_handle],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("public operation handle is unknown"))?;
        let status = self.durable_operation_status(operation_handle)?;
        if persisted.5.as_deref() != Some("approved")
            || persisted.6.as_deref() != Some("needs_approval")
        {
            let replayable_failed_submit = persisted.0 == "hubu_submit_spend"
                && persisted.6.as_deref() == Some("allow")
                && status.result_code.as_deref() == Some("spend_failed");
            if (status.state != "failed" || replayable_failed_submit)
                && matches!(persisted.6.as_deref(), Some("allow" | "deny"))
            {
                if let Some(result_json) = persisted.7.as_deref() {
                    let authoritative_result = serde_json::from_str(result_json)
                        .context("decode completed operation resume result")?;
                    return Ok(ResumePreparation::Replay {
                        origin: resume_origin(&persisted.0)?,
                        authoritative_result,
                        status,
                    });
                }
            }
            return Ok(ResumePreparation::Status(status));
        }
        if status.terminal() {
            return Ok(ResumePreparation::Status(status));
        }
        let Some(request_json) = persisted.4.as_deref() else {
            let failed =
                self.fail_pre_execution_operation(operation_handle, "resume_intent_unavailable")?;
            return Ok(ResumePreparation::IntentUnavailable(failed));
        };
        let request: Value = serde_json::from_str(request_json)
            .context("decode stored normalized operation intent")?;
        validate_durable_request_size(&request)?;
        if canonical_request_hash(&persisted.0, &request)? != persisted.3 {
            bail!("stored normalized operation intent conflicts with its canonical request hash");
        }
        let origin = resume_origin(&persisted.0)?;
        let hubu_arguments = match origin {
            ResumeOrigin::AuthorizeSpend | ResumeOrigin::SubmitSpend => request,
            ResumeOrigin::GovernedExecution => {
                crate::governed_execution::resume_authorization_arguments(&request)?
            }
        };
        let operation_key = persisted
            .1
            .ok_or_else(|| anyhow!("normalized operation is missing private operation identity"))?;
        Ok(ResumePreparation::Dispatch(ResumeOperationPlan {
            origin,
            operation: OperationResolution {
                operation_key: Some(operation_key),
                operation_handle: operation_handle.to_owned(),
                task_id: persisted.2,
                recorded_result: None,
            },
            hubu_arguments,
        }))
    }

    pub(crate) fn complete_resume(
        &mut self,
        plan: &ResumeOperationPlan,
        result: &Value,
    ) -> Result<ResumeCompletion> {
        if contains_protected_identity(result) {
            bail!("resumed normalized operation result contains protected backend identity");
        }
        let mut authoritative_result = result.clone();
        let mut incoming_result_json = serde_json::to_string(&authoritative_result)?;
        if incoming_result_json.len() > MAX_RESULT_BYTES {
            bail!("resumed normalized operation result is too large to persist safely");
        }
        let reported_decision = result
            .get("decision")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("resumed Hubu result is missing decision"))?;
        if !matches!(reported_decision, "allow" | "deny" | "needs_approval") {
            bail!("resumed Hubu result has an unsupported decision");
        }
        let terminal_submit_payment = if plan.origin == ResumeOrigin::SubmitSpend {
            terminal_payment_receipt(result)
        } else {
            None
        };
        let incoming_decision_id = optional_string(result, "decision_id")?;
        let mut continuation_invalid = false;
        let auth_token_id = match optional_string(result, "auth_token_id") {
            Ok(value) => value,
            Err(_) => {
                continuation_invalid = true;
                None
            }
        };
        let spend_auth_token_id = match optional_string(result, "spend_auth_token_id") {
            Ok(value) => value,
            Err(_) => {
                continuation_invalid = true;
                None
            }
        };
        if auth_token_id
            .as_deref()
            .zip(spend_auth_token_id.as_deref())
            .is_some_and(|(left, right)| left != right)
        {
            continuation_invalid = true;
        }
        let incoming_auth_token_id = auth_token_id.or(spend_auth_token_id);
        let governed_allow =
            plan.origin == ResumeOrigin::GovernedExecution && reported_decision == "allow";
        let authorization_allow = matches!(
            plan.origin,
            ResumeOrigin::AuthorizeSpend | ResumeOrigin::GovernedExecution
        ) && reported_decision == "allow";
        if governed_allow
            && incoming_auth_token_id
                .as_deref()
                .is_none_or(|auth_token_id| {
                    validate_identifier("auth_token_id", auth_token_id, 255).is_err()
                })
        {
            continuation_invalid = true;
        }
        if continuation_invalid && !governed_allow {
            bail!("resumed Hubu result has an invalid authorization continuation");
        }
        let incoming_approval_request_id = result
            .get("approval")
            .and_then(|approval| approval.get("approval_request_id"))
            .filter(|value| !value.is_null())
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| anyhow!("approval_request_id must be a string"))
            })
            .transpose()?;
        if let Some(approval_request_id) = incoming_approval_request_id.as_deref() {
            validate_identifier("approval_request_id", approval_request_id, 255)?;
        }
        let incoming_authorization_expires_at =
            optional_string(result, "authorization_expires_at")?;
        let parsed_authorization_expiry = incoming_authorization_expires_at
            .as_deref()
            .and_then(|expires_at| DateTime::parse_from_rfc3339(expires_at).ok())
            .map(|expires_at| expires_at.with_timezone(&Utc));
        if governed_allow
            && incoming_authorization_expires_at.is_some()
            && parsed_authorization_expiry.is_none()
        {
            continuation_invalid = true;
        }
        let authorization_expired = authorization_allow
            && !continuation_invalid
            && parsed_authorization_expiry.is_some_and(|expires_at| expires_at <= Utc::now());
        if authorization_expired {
            let object = authoritative_result
                .as_object_mut()
                .expect("a validated resumed Hubu result is an object");
            object.remove("auth_token_id");
            object.remove("spend_auth_token_id");
            object.insert("requires_human_approval".into(), Value::Bool(false));
            object.insert(
                "retry_guidance".into(),
                json!({
                    "action": "create_new_operation",
                    "message": "The prior approved authorization expired before resume. Create a new logical operation; the expired operation cannot be resumed."
                }),
            );
            incoming_result_json = serde_json::to_string(&authoritative_result)?;
            if incoming_result_json.len() > MAX_RESULT_BYTES {
                bail!("resumed normalized operation result is too large to persist safely");
            }
        }
        let persisted_auth_token_id = if authorization_expired {
            None
        } else {
            incoming_auth_token_id.clone()
        };

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = transaction
            .query_row(
                "SELECT tool_name, operation_key, task_id, request_hash,
                        normalized_request_json, approval_request_id, approval_status,
                        decision, result_json, operation_state, operation_result_code,
                        gongbu_request_hash, gongbu_execution_id
                 FROM harness_operations WHERE operation_handle = ?1",
                [&plan.operation.operation_handle],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, Option<String>>(9)?,
                        row.get::<_, Option<String>>(10)?,
                        row.get::<_, Option<String>>(11)?,
                        row.get::<_, Option<String>>(12)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("resumed operation has no matching normalized operation"))?;
        let origin = resume_origin(&existing.0)?;
        if origin != plan.origin
            || plan.origin.normalized_tool_name() != existing.0
            || existing.1 != plan.operation.operation_key
            || existing.2 != plan.operation.task_id
        {
            bail!("resume plan conflicts with its normalized operation identity");
        }
        if existing
            .5
            .as_deref()
            .zip(incoming_approval_request_id.as_deref())
            .is_some_and(|(stored, incoming)| stored != incoming)
        {
            bail!("resumed Hubu result conflicts with its approval request");
        }
        if let Some(operation_key) = existing.1.as_deref() {
            if incoming_result_json.contains(operation_key) {
                bail!("resumed normalized operation result contains private backend identity");
            }
        }

        let may_supersede_expired_submit = origin == ResumeOrigin::SubmitSpend
            && terminal_submit_payment.is_some()
            && existing.6.as_deref() == Some("approved")
            && existing.9.as_deref() == Some("failed")
            && existing.10.as_deref() == Some("authorization_expired_before_resume")
            && existing.12.is_none();
        let existing_is_complete = matches!(existing.7.as_deref(), Some("allow" | "deny"))
            && !(origin == ResumeOrigin::GovernedExecution
                && existing.7.as_deref() == Some("allow")
                && existing.9.is_none());
        if (existing_is_complete || existing.9.as_deref().is_some_and(is_durable_terminal_state))
            && !may_supersede_expired_submit
        {
            let authoritative_result = existing
                .8
                .as_deref()
                .ok_or_else(|| anyhow!("terminal normalized operation is missing replay state"))
                .and_then(|value| {
                    serde_json::from_str(value)
                        .context("decode terminal normalized operation resume result")
                })?;
            transaction.commit()?;
            let status = self.durable_operation_status(&plan.operation.operation_handle)?;
            return Ok(ResumeCompletion {
                authoritative_result,
                status,
                wake_operation_worker: false,
            });
        }

        let established_approval = monotonic_approval_status(
            existing.6.as_deref(),
            existing.7.as_deref().and_then(approval_status_for_decision),
        )?;
        let mut incoming_decision = reported_decision;
        let mut completed_submit_payment = None;
        if origin == ResumeOrigin::SubmitSpend {
            match (reported_decision, terminal_submit_payment) {
                ("deny", Some(_)) => {
                    bail!("resumed Hubu spend result conflicts with its terminal payment receipt")
                }
                ("allow", None) => {
                    bail!("resumed Hubu spend allow is missing a valid terminal payment receipt")
                }
                ("allow" | "needs_approval", Some(receipt)) => {
                    if existing.6.as_deref() != Some("approved") {
                        bail!("resumed Hubu spend payment is missing durable approval")
                    }
                    incoming_decision = "allow";
                    completed_submit_payment = Some(receipt);
                    let object = authoritative_result
                        .as_object_mut()
                        .expect("a validated resumed Hubu result is an object");
                    object.insert("decision".into(), Value::String("allow".into()));
                    object.insert("requires_human_approval".into(), Value::Bool(false));
                    object.remove("approval_reason");
                    object.remove("retry_guidance");
                    incoming_result_json = serde_json::to_string(&authoritative_result)?;
                    if incoming_result_json.len() > MAX_RESULT_BYTES {
                        bail!("resumed normalized operation result is too large to persist safely");
                    }
                }
                ("needs_approval" | "deny", None) => {}
                _ => unreachable!("resumed Hubu decision was validated above"),
            }
        }
        let has_approval = established_approval.is_some()
            || existing.5.is_some()
            || incoming_approval_request_id.is_some();
        let incoming_approval = has_approval
            .then(|| approval_status_for_decision(incoming_decision))
            .flatten();
        let approval_status =
            monotonic_approval_status(established_approval.as_deref(), incoming_approval)?;

        let mut gongbu_request_hash = None;
        let mut gongbu_request_json = None;
        if origin == ResumeOrigin::GovernedExecution
            && incoming_decision == "allow"
            && !continuation_invalid
            && !authorization_expired
        {
            let auth_token_id = incoming_auth_token_id
                .as_deref()
                .expect("validated governed continuation is present");
            let request_json = existing
                .4
                .as_deref()
                .ok_or_else(|| anyhow!("resume_intent_unavailable"))?;
            let request: Value = serde_json::from_str(request_json)
                .context("decode stored governed operation resume intent")?;
            if canonical_request_hash(&existing.0, &request)? != existing.3 {
                bail!("stored governed operation resume intent conflicts with its canonical request hash");
            }
            let arguments =
                crate::governed_execution::resume_execution_arguments(&request, auth_token_id)?;
            let request_hash = canonical_request_hash("gongbu_create_execution", &arguments)?;
            if existing
                .11
                .as_deref()
                .is_some_and(|stored| stored != request_hash)
            {
                bail!("resumed governed execution conflicts with its bound execution intent");
            }
            gongbu_request_hash = Some(request_hash);
            gongbu_request_json = Some(serde_json::to_string(&canonicalize(&arguments))?);
        }

        let clear_normalized_request = incoming_decision == "deny"
            || (incoming_decision == "allow" && origin != ResumeOrigin::GovernedExecution)
            || completed_submit_payment.is_some()
            || gongbu_request_json.is_some()
            || continuation_invalid
            || authorization_expired;
        let changed = transaction.execute(
            "UPDATE harness_operations
             SET decision = ?2,
                 decision_id = ?3,
                 auth_token_id = ?4,
                 approval_request_id = COALESCE(?5, approval_request_id),
                 approval_status = COALESCE(?6, approval_status),
                 approval_synced_at = CASE WHEN ?6 IS NULL THEN approval_synced_at ELSE CURRENT_TIMESTAMP END,
                 authorization_expires_at = ?7,
                 result_json = ?8,
                 result_recorded_at = CURRENT_TIMESTAMP,
                 normalized_request_json = CASE WHEN ?9 THEN NULL ELSE normalized_request_json END,
                 gongbu_request_hash = COALESCE(gongbu_request_hash, ?10),
                 gongbu_request_json = CASE
                     WHEN operation_state IN ('succeeded','failed') OR gongbu_execution_id IS NOT NULL
                         THEN gongbu_request_json
                     ELSE COALESCE(gongbu_request_json, ?11)
                 END,
                 operation_state = CASE
                     WHEN ?15 IS NOT NULL THEN ?15
                     WHEN ?14 THEN 'failed'
                     WHEN ?13 THEN 'failed'
                     WHEN ?11 IS NULL THEN operation_state
                     ELSE COALESCE(operation_state, 'accepted')
                 END,
                 operation_result_code = CASE
                     WHEN ?16 IS NOT NULL THEN ?16
                     WHEN ?14 THEN 'authorization_expired_before_resume'
                     WHEN ?13 THEN 'authorization_continuation_unavailable'
                     ELSE operation_result_code
                 END,
                 operation_deadline_at = CASE
                     WHEN ?11 IS NULL THEN operation_deadline_at
                     ELSE COALESCE(
                         operation_deadline_at,
                         strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+' || ?12 || ' hours')
                     )
                 END,
                 gongbu_create_started_at = CASE
                     WHEN ?11 IS NULL THEN gongbu_create_started_at
                     ELSE COALESCE(gongbu_create_started_at, CURRENT_TIMESTAMP)
                 END,
                 operation_updated_at = CASE
                     WHEN ?11 IS NULL AND NOT ?13 AND NOT ?14 AND ?15 IS NULL
                         THEN operation_updated_at
                     ELSE CURRENT_TIMESTAMP
                 END
             WHERE operation_handle = ?1
               AND (gongbu_request_hash IS NULL OR gongbu_request_hash = ?10)",
            params![
                plan.operation.operation_handle,
                incoming_decision,
                incoming_decision_id,
                persisted_auth_token_id,
                incoming_approval_request_id,
                approval_status,
                incoming_authorization_expires_at,
                incoming_result_json,
                clear_normalized_request,
                gongbu_request_hash,
                gongbu_request_json,
                OPERATION_DEADLINE_HOURS,
                continuation_invalid,
                authorization_expired,
                completed_submit_payment.map(TerminalPaymentReceipt::operation_state),
                completed_submit_payment.map(TerminalPaymentReceipt::result_code),
            ],
        )?;
        if changed != 1 {
            bail!("resumed operation conflicts with its durable normalized state");
        }
        transaction.commit()?;
        let status = self.durable_operation_status(&plan.operation.operation_handle)?;
        Ok(ResumeCompletion {
            authoritative_result,
            wake_operation_worker: origin == ResumeOrigin::GovernedExecution
                && incoming_decision == "allow"
                && !continuation_invalid
                && !authorization_expired
                && !status.terminal(),
            status,
        })
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
                 gongbu_create_started_at = COALESCE(gongbu_create_started_at, CURRENT_TIMESTAMP),
                 normalized_request_json = NULL
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
                        authorization_expires_at, approval_status,
                        normalized_request_json IS NOT NULL,
                        COALESCE(operation_updated_at, approval_synced_at,
                                 result_recorded_at, dispatch_started_at, created_at)
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
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, bool>(9)?,
                        row.get::<_, String>(10)?,
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
                persisted.8.as_deref(),
                persisted.9,
            )?,
        };
        Ok(DurableOperationStatus {
            operation_handle: persisted.0,
            state,
            execution_id: persisted.2,
            result_code,
            updated_at: persisted.10,
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
                 normalized_request_json = NULL, gongbu_request_json = NULL,
                 next_operation_attempt_at = NULL,
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

fn valid_terminal_governed_result(value: &Value, operation_handle: &str) -> bool {
    !contains_protected_identity(value)
        && value
            .pointer("/structuredContent/operation_handle")
            .and_then(Value::as_str)
            == Some(operation_handle)
        && value
            .pointer("/structuredContent/terminal")
            .and_then(Value::as_bool)
            == Some(true)
}

struct PreallocatedOperationKey {
    record_id: String,
    operation_key: String,
}

fn preallocated_operation_key(
    path: &Path,
    tool_name: &str,
    arguments: &Value,
    installation_id: &str,
    identity: &NormalizedHarnessIdentity,
    request_hash: &str,
) -> Result<PreallocatedOperationKey> {
    validate_preallocated_store_files(path)?;
    let binding_id = preallocated_binding_id(installation_id, identity, request_hash)?;

    let mut connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|_| anyhow!("preallocated operation key store is unavailable"))?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|_| anyhow!("preallocated operation key store is unavailable"))?;

    let expected_scope = canonicalize(&json!({
        "schema_version": 1,
        "tool_name": tool_name,
        "arguments": canonicalize(arguments),
    }));
    let expected_scope_json = serde_json::to_string(&expected_scope)?;
    let expected_scope_sha256 = format!("{:x}", Sha256::digest(expected_scope_json.as_bytes()));

    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| anyhow!("preallocated operation key store is unavailable"))?;
    let rows = {
        let mut statement = transaction
            .prepare(
                "SELECT record_id, operation_key, scope_json, scope_sha256,
                        binding_id, bound_at
                 FROM operations
                 WHERE status = 'active' AND (scope_sha256 = ?1 OR scope_json = ?2)
                 ORDER BY record_id
                 LIMIT 2",
            )
            .map_err(|_| anyhow!("preallocated operation key store schema is invalid"))?;
        statement
            .query_map(params![expected_scope_sha256, expected_scope_json], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })
            .and_then(Iterator::collect::<rusqlite::Result<Vec<_>>>)
            .map_err(|_| anyhow!("preallocated operation key store is unreadable"))?
    };
    let [(record_id, operation_key, scope_json, scope_sha256, stored_binding, bound_at)] =
        rows.as_slice()
    else {
        bail!("exactly one active preallocated operation key record is required");
    };

    let stored_scope_sha256 = format!("{:x}", Sha256::digest(scope_json.as_bytes()));
    if scope_json != &expected_scope_json
        || scope_sha256 != &expected_scope_sha256
        || scope_sha256 != &stored_scope_sha256
        || !valid_preallocated_identifiers(record_id, operation_key)
        || stored_binding
            .as_ref()
            .is_some_and(|stored| stored != &binding_id)
        || stored_binding.is_some() != bound_at.is_some()
    {
        bail!("preallocated operation key record scope or identity is invalid");
    }

    if stored_binding.is_none() {
        let changed = transaction
            .execute(
                "UPDATE operations
                 SET binding_id = ?2, bound_at = ?3, updated_at = ?3
                 WHERE record_id = ?1 AND status = 'active' AND binding_id IS NULL",
                params![
                    record_id,
                    binding_id,
                    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
                ],
            )
            .map_err(|_| anyhow!("preallocated operation key record could not be claimed"))?;
        if changed != 1 {
            bail!("preallocated operation key record could not be claimed");
        }
    }
    transaction
        .commit()
        .map_err(|_| anyhow!("preallocated operation key record could not be claimed"))?;
    secure_preallocated_store_sidecars(path)?;

    Ok(PreallocatedOperationKey {
        record_id: record_id.clone(),
        operation_key: operation_key.clone(),
    })
}

fn preallocated_binding_id(
    installation_id: &str,
    identity: &NormalizedHarnessIdentity,
    request_hash: &str,
) -> Result<String> {
    let projection = canonicalize(&json!({
        "schema_version": 1,
        "installation_id": installation_id,
        "platform": identity.platform,
        "harness_call_id": identity.harness_call_id,
        "request_hash": request_hash,
    }));
    let serialized = serde_json::to_vec(&projection)?;
    Ok(format!(
        "hubu:preallocated-binding:v1:{:x}",
        Sha256::digest(serialized)
    ))
}

fn validate_preallocated_store_files(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| anyhow!("preallocated operation key store is unavailable"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("preallocated operation key store is unavailable");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            bail!("preallocated operation key store permissions are unsafe");
        }
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("preallocated operation key store is unavailable"))?;
        let parent_metadata = fs::symlink_metadata(parent)
            .map_err(|_| anyhow!("preallocated operation key store is unavailable"))?;
        if parent_metadata.file_type().is_symlink()
            || !parent_metadata.is_dir()
            || parent_metadata.permissions().mode() & 0o077 != 0
        {
            bail!("preallocated operation key store directory permissions are unsafe");
        }
        for sidecar in preallocated_store_sidecars(path) {
            let Ok(sidecar_metadata) = fs::symlink_metadata(&sidecar) else {
                continue;
            };
            if sidecar_metadata.file_type().is_symlink()
                || !sidecar_metadata.is_file()
                || sidecar_metadata.permissions().mode() & 0o077 != 0
            {
                bail!("preallocated operation key store sidecar permissions are unsafe");
            }
        }
    }
    Ok(())
}

fn preallocated_store_sidecars(path: &Path) -> [PathBuf; 2] {
    let with_suffix = |suffix: &str| {
        let mut value = path.as_os_str().to_os_string();
        value.push(suffix);
        PathBuf::from(value)
    };
    [with_suffix("-wal"), with_suffix("-shm")]
}

fn secure_preallocated_store_sidecars(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for sidecar in preallocated_store_sidecars(path) {
            let Ok(metadata) = fs::symlink_metadata(&sidecar) else {
                continue;
            };
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                bail!("preallocated operation key store sidecar permissions are unsafe");
            }
            fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o600))
                .map_err(|_| anyhow!("preallocated operation key store sidecar is unavailable"))?;
        }
    }
    Ok(())
}

fn valid_preallocated_identifiers(record_id: &str, operation_key: &str) -> bool {
    let Some(record_suffix) = record_id.strip_prefix("hop_") else {
        return false;
    };
    let Some(key_suffix) = operation_key.strip_prefix("codex:v1:") else {
        return false;
    };
    valid_lower_hex_identifier(record_suffix)
        && valid_lower_hex_identifier(key_suffix)
        && record_suffix != key_suffix
}

fn valid_lower_hex_identifier(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn migrate_v4_to_v5(connection: &mut Connection) -> Result<()> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("begin unified MCP operation registry v4 to v5 migration")?;
    transaction
        .execute_batch(
            "ALTER TABLE harness_operations ADD COLUMN normalized_request_json TEXT
                 CHECK(normalized_request_json IS NULL OR
                       (json_valid(normalized_request_json) AND length(normalized_request_json) <= 1048576));
             ALTER TABLE harness_operations ADD COLUMN approval_status TEXT
                 CHECK(approval_status IS NULL OR approval_status IN ('pending','approved','denied'));
             ALTER TABLE harness_operations ADD COLUMN approval_synced_at TEXT;
             UPDATE harness_operations
                SET approval_status = 'pending',
                    approval_synced_at = COALESCE(result_recorded_at, created_at)
              WHERE decision = 'needs_approval' AND approval_request_id IS NOT NULL;
             CREATE UNIQUE INDEX harness_operation_approval_request
                 ON harness_operations(approval_request_id)
                 WHERE approval_request_id IS NOT NULL;",
        )
        .context("migrate unified MCP operation registry approval continuation schema")?;
    transaction.pragma_update(None, "user_version", PREVIOUS_SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v5_to_v6(connection: &mut Connection) -> Result<()> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("begin unified MCP operation registry v5 to v6 migration")?;
    transaction
        .execute_batch(
            "ALTER TABLE harness_operations ADD COLUMN operation_key_record_id TEXT
                 CHECK(operation_key_record_id IS NULL OR length(operation_key_record_id) = 36);
             ALTER TABLE harness_operations ADD COLUMN governed_result_json TEXT
                 CHECK(governed_result_json IS NULL OR
                       (json_valid(governed_result_json) AND length(governed_result_json) <= 16777216));
             CREATE UNIQUE INDEX harness_operation_key_record
                 ON harness_operations(operation_key_record_id)
                 WHERE operation_key_record_id IS NOT NULL;",
        )
        .context("migrate unified MCP preallocated operation key binding schema")?;
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
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
             normalized_request_json TEXT CHECK(normalized_request_json IS NULL OR (json_valid(normalized_request_json) AND length(normalized_request_json) <= 1048576)),
             tool_name TEXT NOT NULL CHECK(length(tool_name) BETWEEN 1 AND 128),
             operation_key TEXT UNIQUE CHECK(operation_key IS NULL OR length(operation_key) BETWEEN 1 AND 160),
             operation_key_record_id TEXT CHECK(operation_key_record_id IS NULL OR length(operation_key_record_id) = 36),
             governed_result_json TEXT CHECK(governed_result_json IS NULL OR (json_valid(governed_result_json) AND length(governed_result_json) <= 16777216)),
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
             approval_status TEXT CHECK(approval_status IS NULL OR approval_status IN ('pending','approved','denied')),
             approval_synced_at TEXT,
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
         CREATE UNIQUE INDEX harness_operation_key_record
             ON harness_operations(operation_key_record_id)
             WHERE operation_key_record_id IS NOT NULL;
         CREATE UNIQUE INDEX harness_operation_approval_request
             ON harness_operations(approval_request_id) WHERE approval_request_id IS NOT NULL;
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

fn validate_approval_status(status: &str) -> Result<()> {
    if matches!(status, "pending" | "approved" | "denied") {
        Ok(())
    } else {
        bail!("Hubu returned an unsupported approval status")
    }
}

fn approval_status_for_decision(decision: &str) -> Option<&'static str> {
    match decision {
        "needs_approval" => Some("pending"),
        "allow" => Some("approved"),
        "deny" => Some("denied"),
        _ => None,
    }
}

fn is_durable_terminal_state(state: &str) -> bool {
    matches!(state, "succeeded" | "failed")
}

fn monotonic_approval_status(
    existing: Option<&str>,
    incoming: Option<&str>,
) -> Result<Option<String>> {
    if let Some(existing) = existing {
        validate_approval_status(existing)?;
    }
    if let Some(incoming) = incoming {
        validate_approval_status(incoming)?;
    }
    match (existing, incoming) {
        (None, None) => Ok(None),
        (Some(existing), None) => Ok(Some(existing.to_owned())),
        (None, Some(incoming)) => Ok(Some(incoming.to_owned())),
        (Some(existing), Some(incoming)) if existing == incoming => Ok(Some(existing.to_owned())),
        (Some("pending"), Some(incoming @ ("approved" | "denied"))) => {
            Ok(Some(incoming.to_owned()))
        }
        (Some(existing @ ("approved" | "denied")), Some("pending")) => {
            Ok(Some(existing.to_owned()))
        }
        (Some("approved"), Some("denied")) | (Some("denied"), Some("approved")) => {
            bail!("approval status conflicts with its durable terminal resolution")
        }
        _ => bail!("approval status transition is unsupported"),
    }
}

fn resume_origin(tool_name: &str) -> Result<ResumeOrigin> {
    match tool_name {
        "hubu_authorize_spend" => Ok(ResumeOrigin::AuthorizeSpend),
        "hubu_submit_spend" => Ok(ResumeOrigin::SubmitSpend),
        crate::governed_execution::TOOL_NAME => Ok(ResumeOrigin::GovernedExecution),
        _ => bail!("normalized operation does not support approval resume"),
    }
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
    approval_status: Option<&str>,
    has_resume_intent: bool,
) -> Result<(String, Option<String>)> {
    let projection = match (
        tool_name,
        decision,
        has_authorization,
        approval_status,
        has_resume_intent,
    ) {
        ("hubu_authorize_spend" | "hubu_submit_governed_execution", Some("allow"), true, _, _) => {
            ("authorized", None)
        }
        ("hubu_authorize_spend" | "hubu_submit_governed_execution", Some("allow"), false, _, _) => {
            ("failed", Some("authorization_continuation_unavailable"))
        }
        ("hubu_authorize_spend" | "hubu_submit_governed_execution", Some("deny"), _, _, _) => {
            ("failed", Some("authorization_denied"))
        }
        ("hubu_submit_spend", Some("allow"), _, _, _) => ("succeeded", Some("spend_succeeded")),
        ("hubu_submit_spend", Some("deny"), _, _, _) => ("failed", Some("spend_denied")),
        (
            "hubu_authorize_spend" | "hubu_submit_spend" | "hubu_submit_governed_execution",
            Some("needs_approval"),
            _,
            Some("approved"),
            true,
        ) => ("resume_required", Some("approval_resolved_resume_required")),
        (
            "hubu_authorize_spend" | "hubu_submit_spend" | "hubu_submit_governed_execution",
            Some("needs_approval"),
            _,
            Some("approved"),
            false,
        ) => ("resume_required", Some("resume_intent_unavailable")),
        (
            "hubu_authorize_spend" | "hubu_submit_spend" | "hubu_submit_governed_execution",
            Some("needs_approval"),
            _,
            Some("denied"),
            _,
        ) => ("failed", Some("approval_denied")),
        (
            "hubu_authorize_spend" | "hubu_submit_spend" | "hubu_submit_governed_execution",
            Some("needs_approval"),
            _,
            None | Some("pending"),
            _,
        ) => ("approval_required", Some("human_approval_required")),
        (
            "hubu_authorize_spend" | "hubu_submit_spend" | "hubu_submit_governed_execution",
            None,
            _,
            _,
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

fn terminal_payment_receipt(value: &Value) -> Option<TerminalPaymentReceipt> {
    let payment = value.get("payment").and_then(Value::as_object)?;
    if payment
        .get("payment_id")
        .and_then(Value::as_str)
        .is_none_or(|payment_id| validate_identifier("payment_id", payment_id, 255).is_err())
    {
        return None;
    }
    match payment.get("status").and_then(Value::as_str) {
        Some("succeeded") => Some(TerminalPaymentReceipt::Succeeded),
        Some("failed") => Some(TerminalPaymentReceipt::Failed),
        _ => None,
    }
}

fn validate_schema(connection: &Connection) -> Result<()> {
    connection
        .prepare("SELECT singleton, installation_id, created_at FROM installation_identity LIMIT 0")
        .context("validate unified MCP operation registry installation schema")?;
    connection
        .prepare(
            "SELECT platform, installation_id, harness_call_id, request_hash,
                    normalized_request_json, tool_name, operation_key, operation_key_record_id,
                    governed_result_json,
                    operation_handle,
                    codex_call_id, claude_tool_use_id, hubu_invocation_id,
                    controlled_installation_id, task_id, decision, decision_id,
                    auth_token_id, approval_request_id, approval_status, approval_synced_at,
                    authorization_expires_at,
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
    use std::{process::Command, sync::Barrier, thread};

    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    fn codex(call_id: &str) -> NormalizedHarnessIdentity {
        NormalizedHarnessIdentity::from_meta(Some(&json!({"callId": call_id}))).unwrap()
    }

    fn preallocated_scope(tool_name: &str, arguments: &Value) -> (String, String) {
        let scope = canonicalize(&json!({
            "schema_version": 1,
            "tool_name": tool_name,
            "arguments": canonicalize(arguments),
        }));
        let scope_json = serde_json::to_string(&scope).unwrap();
        let scope_sha256 = format!("{:x}", Sha256::digest(scope_json.as_bytes()));
        (scope_json, scope_sha256)
    }

    fn create_preallocated_store(path: &Path) -> Connection {
        let connection = Connection::open(path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        connection
            .execute_batch(
                "CREATE TABLE operations (
                     record_id TEXT PRIMARY KEY,
                     operation_key TEXT NOT NULL UNIQUE,
                     label TEXT NOT NULL,
                     scope_json TEXT NOT NULL,
                     scope_sha256 TEXT NOT NULL,
                     status TEXT NOT NULL,
                     binding_id TEXT UNIQUE,
                     bound_at TEXT,
                     created_at TEXT NOT NULL,
                     updated_at TEXT NOT NULL
                 );",
            )
            .unwrap();
        connection
    }

    fn insert_preallocated_record(
        connection: &Connection,
        suffix: &str,
        status: &str,
        scope_json: &str,
        scope_sha256: &str,
        operation_key: Option<&str>,
    ) {
        let derived_operation_key = || {
            let digest = format!(
                "{:x}",
                Sha256::digest(format!("fixed-fixture-key:{suffix}").as_bytes())
            );
            format!("codex:v1:{}", &digest[..32])
        };
        connection
            .execute(
                "INSERT INTO operations(
                     record_id, operation_key, label, scope_json, scope_sha256,
                     status, binding_id, bound_at, created_at, updated_at
                 ) VALUES (?1, ?2, 'fixed test fixture', ?3, ?4, ?5,
                           NULL, NULL, '2026-08-28T00:00:00Z', '2026-08-28T00:00:00Z')",
                params![
                    format!("hop_{suffix}"),
                    operation_key
                        .map(str::to_owned)
                        .unwrap_or_else(derived_operation_key),
                    scope_json,
                    scope_sha256,
                    status,
                ],
            )
            .unwrap();
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

    fn governed_arguments() -> Value {
        json!({
            "authorization": {
                "account_id": "account-1",
                "amount_cents": 100,
                "reason": "generate one image"
            },
            "execution": {
                "schema_version": 2,
                "input": {"prompt": "durable prompt", "image_count": 1},
                "input_schema_version": 1,
                "workload_type": "image_generation",
                "provider": "fixture",
                "adapter": "fixture",
                "model": "fixture-v1"
            },
            "max_inline_artifact_bytes": 1024
        })
    }

    fn pending_approval(
        registry: &mut OperationRegistry,
        call_id: &str,
        tool_name: &str,
        arguments: &Value,
        approval_request_id: &str,
    ) -> OperationResolution {
        let operation = registry
            .resolve_or_allocate(&codex(call_id), tool_name, arguments)
            .unwrap();
        registry
            .record_authorization_result(
                &operation.operation_handle,
                &json!({
                    "decision": "needs_approval",
                    "decision_id": approval_request_id,
                    "approval": {
                        "approval_request_id": approval_request_id,
                        "status": "pending"
                    },
                    "operation_handle": operation.operation_handle
                }),
            )
            .unwrap();
        operation
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
    fn preallocated_operation_key_binds_once_and_replays_after_restart() {
        let root = tempdir().unwrap();
        let registry_path = root.path().join("router.sqlite3");
        let helper_path = root.path().join("operation-keys.sqlite3");
        let arguments = json!({"z": [2, 1], "a": {"nested": true}});
        let (scope_json, scope_sha256) =
            preallocated_scope(crate::governed_execution::TOOL_NAME, &arguments);
        let helper = create_preallocated_store(&helper_path);
        let suffix = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let operation_key = "codex:v1:11111111111111111111111111111111";
        insert_preallocated_record(
            &helper,
            suffix,
            "active",
            &scope_json,
            &scope_sha256,
            Some(operation_key),
        );
        drop(helper);

        let mut registry =
            OperationRegistry::open_with_preallocated_keys(&registry_path, Some(&helper_path))
                .unwrap();
        let first = registry
            .resolve_or_allocate(
                &codex("preallocated-call"),
                crate::governed_execution::TOOL_NAME,
                &arguments,
            )
            .unwrap();
        assert_eq!(first.operation_key.as_deref(), Some(operation_key));
        assert_eq!(
            registry
                .connection
                .query_row(
                    "SELECT operation_key_record_id FROM harness_operations",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "hop_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        let helper_after_claim = Connection::open(&helper_path).unwrap();
        let (binding_id, bound_at) = helper_after_claim
            .query_row("SELECT binding_id, bound_at FROM operations", [], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            })
            .unwrap();
        assert!(binding_id
            .as_deref()
            .is_some_and(|value| value.starts_with("hubu:preallocated-binding:v1:")));
        assert!(bound_at.is_some());
        drop(helper_after_claim);

        let competing_registry_path = root.path().join("competing-router.sqlite3");
        let mut competing = OperationRegistry::open_with_preallocated_keys(
            &competing_registry_path,
            Some(&helper_path),
        )
        .unwrap();
        let competing_error = competing
            .resolve_or_allocate(
                &codex("preallocated-call"),
                crate::governed_execution::TOOL_NAME,
                &arguments,
            )
            .unwrap_err()
            .to_string();
        assert!(competing_error.contains("already bound or invalid"));
        assert_eq!(
            competing
                .connection
                .query_row("SELECT COUNT(*) FROM harness_operations", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
        drop(competing);

        let error = registry
            .resolve_or_allocate(
                &codex("different-call"),
                crate::governed_execution::TOOL_NAME,
                &arguments,
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("already bound or invalid"));
        assert!(!error.contains("codex:v1:"));
        drop(registry);

        fs::rename(&helper_path, root.path().join("operation-keys.offline")).unwrap();
        let mut restarted =
            OperationRegistry::open_with_preallocated_keys(&registry_path, Some(&helper_path))
                .unwrap();
        let replay = restarted
            .resolve_or_allocate(
                &codex("preallocated-call"),
                crate::governed_execution::TOOL_NAME,
                &json!({"a": {"nested": true}, "z": [2, 1]}),
            )
            .unwrap();
        assert_eq!(replay, first);
    }

    #[test]
    fn official_helper_record_is_router_compatible_and_public_output_is_redacted() {
        let root = tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let registry_path = root.path().join("router.sqlite3");
        let helper_path = root.path().join("operation-keys.sqlite3");
        let arguments_path = root.path().join("governed-arguments.json");
        let arguments = governed_arguments();
        fs::write(&arguments_path, serde_json::to_vec(&arguments).unwrap()).unwrap();
        let helper_script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../skills/generate-hubu-operation-key/scripts/operation_keys.py");

        let output = Command::new("python3")
            .arg("-B")
            .arg(helper_script)
            .arg("--db")
            .arg(&helper_path)
            .arg("begin-unified")
            .arg("--label")
            .arg("fixed non-billable router compatibility fixture")
            .arg("--tool-name")
            .arg(crate::governed_execution::TOOL_NAME)
            .arg("--arguments-file")
            .arg(&arguments_path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "operation-key helper fixture failed"
        );
        let public_output = String::from_utf8(output.stdout).unwrap();
        assert!(!public_output.contains("operation_key"));
        assert!(!public_output.contains("codex:v1:"));
        assert!(!public_output.contains("arguments"));
        assert!(!public_output.contains(helper_path.to_string_lossy().as_ref()));
        let reference: Value = serde_json::from_str(&public_output).unwrap();
        let record_id = reference["record_id"].as_str().unwrap();

        let mut registry =
            OperationRegistry::open_with_preallocated_keys(&registry_path, Some(&helper_path))
                .unwrap();
        let resolution = registry
            .resolve_or_allocate(
                &codex("official-helper-compatibility"),
                crate::governed_execution::TOOL_NAME,
                &arguments,
            )
            .unwrap();
        let operation_key = resolution.operation_key.as_deref().unwrap();
        assert!(valid_preallocated_identifiers(record_id, operation_key));
        assert!(record_id.strip_prefix("hop_") != operation_key.strip_prefix("codex:v1:"));
    }

    #[test]
    fn official_helper_migrates_legacy_store_without_exposing_private_material() {
        let root = tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let helper_path = root.path().join("legacy-operation-keys.sqlite3");
        let fixed_operation_key = "codex:v1:56565656565656565656565656565656";
        let legacy = Connection::open(&helper_path).unwrap();
        legacy
            .execute_batch(
                "CREATE TABLE operations (
                     record_id TEXT PRIMARY KEY,
                     operation_key TEXT NOT NULL UNIQUE,
                     label TEXT NOT NULL,
                     scope_json TEXT NOT NULL,
                     scope_sha256 TEXT NOT NULL,
                     status TEXT NOT NULL CHECK (status IN ('active', 'completed', 'abandoned')),
                     created_at TEXT NOT NULL,
                     updated_at TEXT NOT NULL
                 );
                 CREATE INDEX operations_status_created
                     ON operations(status, created_at);",
            )
            .unwrap();
        legacy
            .execute(
                "INSERT INTO operations VALUES(
                     'hop_45454545454545454545454545454545', ?1,
                     'fixed legacy migration fixture', '{\"fixture\":true}',
                     'd4e59d060b1f1e3f56fdb81a1e6ffbd6a4e85155f4dfc6f8e2a2f454b62ec223',
                     'active', '2026-08-28T00:00:00Z', '2026-08-28T00:00:00Z'
                 )",
                [fixed_operation_key],
            )
            .unwrap();
        drop(legacy);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&helper_path, fs::Permissions::from_mode(0o600)).unwrap();
        }

        let helper_script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../skills/generate-hubu-operation-key/scripts/operation_keys.py");
        let output = Command::new("python3")
            .arg("-B")
            .arg(helper_script)
            .arg("--db")
            .arg(&helper_path)
            .arg("list")
            .arg("--status")
            .arg("active")
            .arg("--reference-only")
            .output()
            .unwrap();
        assert!(output.status.success(), "legacy helper migration failed");
        let public_output = String::from_utf8(output.stdout).unwrap();
        assert!(!public_output.contains(fixed_operation_key));
        assert!(!public_output.contains("operation_key"));
        assert!(!public_output.contains("scope_json"));
        assert!(!public_output.contains(helper_path.to_string_lossy().as_ref()));

        let migrated = Connection::open(&helper_path).unwrap();
        let columns = migrated
            .prepare("PRAGMA table_info(operations)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(columns.iter().any(|column| column == "binding_id"));
        assert!(columns.iter().any(|column| column == "bound_at"));
        assert_eq!(
            migrated
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_schema
                     WHERE type = 'index' AND name = 'operations_binding'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        let preserved = migrated
            .query_row(
                "SELECT operation_key, binding_id, bound_at FROM operations",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(preserved, (fixed_operation_key.to_owned(), None, None));
        drop(migrated);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in std::iter::once(helper_path.clone())
                .chain(preallocated_store_sidecars(&helper_path))
            {
                if path.exists() {
                    assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o077, 0);
                }
            }
        }
    }

    #[test]
    fn preallocated_operation_key_store_mismatches_fail_before_router_allocation() {
        let arguments = json!({"amount": 3, "currency": "USD"});
        let tool_name = crate::governed_execution::TOOL_NAME;
        let (scope_json, scope_sha256) = preallocated_scope(tool_name, &arguments);

        for case in [
            "missing",
            "inactive",
            "bad-hash",
            "bad-key",
            "coupled-key",
            "duplicate",
        ] {
            let root = tempdir().unwrap();
            let registry_path = root.path().join("router.sqlite3");
            let helper_path = root.path().join("operation-keys.sqlite3");
            let helper = create_preallocated_store(&helper_path);
            match case {
                "missing" => {}
                "inactive" => insert_preallocated_record(
                    &helper,
                    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "completed",
                    &scope_json,
                    &scope_sha256,
                    None,
                ),
                "bad-hash" => insert_preallocated_record(
                    &helper,
                    "cccccccccccccccccccccccccccccccc",
                    "active",
                    &scope_json,
                    &"0".repeat(64),
                    None,
                ),
                "bad-key" => insert_preallocated_record(
                    &helper,
                    "dddddddddddddddddddddddddddddddd",
                    "active",
                    &scope_json,
                    &scope_sha256,
                    Some("not-a-valid-operation-key"),
                ),
                "coupled-key" => insert_preallocated_record(
                    &helper,
                    "abababababababababababababababab",
                    "active",
                    &scope_json,
                    &scope_sha256,
                    Some("codex:v1:abababababababababababababababab"),
                ),
                "duplicate" => {
                    insert_preallocated_record(
                        &helper,
                        "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
                        "active",
                        &scope_json,
                        &scope_sha256,
                        None,
                    );
                    insert_preallocated_record(
                        &helper,
                        "ffffffffffffffffffffffffffffffff",
                        "active",
                        &scope_json,
                        &scope_sha256,
                        None,
                    );
                }
                _ => unreachable!(),
            }
            drop(helper);

            let mut registry =
                OperationRegistry::open_with_preallocated_keys(&registry_path, Some(&helper_path))
                    .unwrap();
            assert!(registry
                .resolve_or_allocate(&codex(case), tool_name, &arguments)
                .is_err());
            assert_eq!(
                registry
                    .connection
                    .query_row("SELECT COUNT(*) FROM harness_operations", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap(),
                0,
                "case {case} must not allocate router state"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn preallocated_operation_key_store_rejects_unsafe_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempdir().unwrap();
        let registry_path = root.path().join("router.sqlite3");
        let helper_path = root.path().join("operation-keys.sqlite3");
        let arguments = json!({"amount": 3});
        let (scope_json, scope_sha256) =
            preallocated_scope(crate::governed_execution::TOOL_NAME, &arguments);
        let helper = create_preallocated_store(&helper_path);
        insert_preallocated_record(
            &helper,
            "99999999999999999999999999999999",
            "active",
            &scope_json,
            &scope_sha256,
            None,
        );
        drop(helper);
        fs::set_permissions(&helper_path, fs::Permissions::from_mode(0o644)).unwrap();

        let mut registry =
            OperationRegistry::open_with_preallocated_keys(&registry_path, Some(&helper_path))
                .unwrap();
        assert!(registry
            .resolve_or_allocate(
                &codex("unsafe-helper-permissions"),
                crate::governed_execution::TOOL_NAME,
                &arguments,
            )
            .unwrap_err()
            .to_string()
            .contains("permissions are unsafe"));
        assert_eq!(
            registry
                .connection
                .query_row("SELECT COUNT(*) FROM harness_operations", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn preallocated_operation_key_store_rejects_unsafe_directory_and_sidecar() {
        use std::os::unix::fs::PermissionsExt;

        let arguments = json!({"amount": 3});
        let tool_name = crate::governed_execution::TOOL_NAME;
        let (scope_json, scope_sha256) = preallocated_scope(tool_name, &arguments);

        let directory_root = tempdir().unwrap();
        let unsafe_parent = directory_root.path().join("unsafe-parent");
        fs::create_dir(&unsafe_parent).unwrap();
        let helper_path = unsafe_parent.join("operation-keys.sqlite3");
        let helper = create_preallocated_store(&helper_path);
        insert_preallocated_record(
            &helper,
            "12121212121212121212121212121212",
            "active",
            &scope_json,
            &scope_sha256,
            None,
        );
        drop(helper);
        fs::set_permissions(&unsafe_parent, fs::Permissions::from_mode(0o755)).unwrap();
        let mut registry = OperationRegistry::open_with_preallocated_keys(
            &directory_root.path().join("router.sqlite3"),
            Some(&helper_path),
        )
        .unwrap();
        assert!(registry
            .resolve_or_allocate(&codex("unsafe-parent"), tool_name, &arguments)
            .unwrap_err()
            .to_string()
            .contains("permissions are unsafe"));
        fs::set_permissions(&unsafe_parent, fs::Permissions::from_mode(0o700)).unwrap();

        let sidecar_root = tempdir().unwrap();
        let sidecar_helper_path = sidecar_root.path().join("operation-keys.sqlite3");
        let helper = create_preallocated_store(&sidecar_helper_path);
        insert_preallocated_record(
            &helper,
            "34343434343434343434343434343434",
            "active",
            &scope_json,
            &scope_sha256,
            None,
        );
        drop(helper);
        let sidecar = preallocated_store_sidecars(&sidecar_helper_path)[0].clone();
        fs::write(&sidecar, b"fixed non-secret sidecar fixture").unwrap();
        fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o644)).unwrap();
        let mut registry = OperationRegistry::open_with_preallocated_keys(
            &sidecar_root.path().join("router.sqlite3"),
            Some(&sidecar_helper_path),
        )
        .unwrap();
        assert!(registry
            .resolve_or_allocate(&codex("unsafe-sidecar"), tool_name, &arguments)
            .unwrap_err()
            .to_string()
            .contains("permissions are unsafe"));
    }

    #[test]
    fn governed_result_cache_is_exact_durable_and_rejects_private_identity() {
        let root = tempdir().unwrap();
        let path = root.path().join("router.sqlite3");
        let request = governed_arguments();
        let (operation, first_result) = {
            let mut registry = OperationRegistry::open(&path).unwrap();
            let operation = registry
                .resolve_or_allocate(
                    &codex("governed-result-cache"),
                    crate::governed_execution::TOOL_NAME,
                    &request,
                )
                .unwrap();
            let first_result = json!({
                "content": [],
                "structuredContent": {
                    "schema_version": 1,
                    "operation_handle": operation.operation_handle,
                    "outcome": "succeeded",
                    "terminal": true,
                    "timing": {"total_ms": 7}
                },
                "isError": false
            });
            assert_eq!(
                registry
                    .record_governed_result(&operation.operation_handle, &first_result)
                    .unwrap(),
                first_result
            );
            assert_eq!(
                registry
                    .record_governed_result(
                        &operation.operation_handle,
                        &json!({
                            "content": [],
                            "structuredContent": {
                                "schema_version": 1,
                                "operation_handle": operation.operation_handle,
                                "outcome": "succeeded",
                                "terminal": true,
                                "timing": {"total_ms": 99}
                            },
                            "isError": false
                        }),
                    )
                    .unwrap(),
                first_result,
                "the first public composite result must win exact redelivery"
            );
            (operation, first_result)
        };

        let restarted = OperationRegistry::open(&path).unwrap();
        assert_eq!(
            restarted
                .governed_result(&operation.operation_handle)
                .unwrap(),
            Some(first_result)
        );
        drop(restarted);

        let mut registry = OperationRegistry::open(&path).unwrap();
        let second = registry
            .resolve_or_allocate(
                &codex("governed-result-private"),
                crate::governed_execution::TOOL_NAME,
                &request,
            )
            .unwrap();
        assert!(registry
            .record_governed_result(
                &second.operation_handle,
                &json!({
                    "content": [],
                    "structuredContent": {
                        "operation_handle": second.operation_handle,
                        "terminal": true,
                        "operation_key": "must-not-persist"
                    },
                    "isError": false
                }),
            )
            .is_err());
        assert!(registry
            .governed_result(&second.operation_handle)
            .unwrap()
            .is_none());
        let private_key = second.operation_key.as_deref().unwrap();
        assert!(registry
            .record_governed_result(
                &second.operation_handle,
                &json!({
                    "content": [],
                    "structuredContent": {
                        "operation_handle": second.operation_handle,
                        "terminal": true,
                        "message": format!("private identity {private_key}")
                    },
                    "isError": false
                }),
            )
            .is_err());

        let third = registry
            .resolve_or_allocate(
                &codex("governed-result-nonterminal"),
                crate::governed_execution::TOOL_NAME,
                &request,
            )
            .unwrap();
        assert!(registry
            .record_governed_result(
                &third.operation_handle,
                &json!({
                    "content": [],
                    "structuredContent": {
                        "operation_handle": third.operation_handle,
                        "outcome": "in_progress",
                        "terminal": false
                    },
                    "isError": false
                }),
            )
            .is_err());
        assert!(registry
            .governed_result(&third.operation_handle)
            .unwrap()
            .is_none());
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
    fn approval_sync_is_correlated_monotonic_and_projects_resume_required() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let arguments = json!({"account_id":"account-1","amount_cents":100,"reason":"test"});
        let operation = pending_approval(
            &mut registry,
            "approval-sync",
            "hubu_authorize_spend",
            &arguments,
            "approval-sync-1",
        );
        assert_eq!(
            registry
                .approval_sync_target_for_handle(&operation.operation_handle)
                .unwrap(),
            Some(ApprovalSyncTarget {
                operation_handle: operation.operation_handle.clone(),
                approval_request_id: "approval-sync-1".into(),
            })
        );
        assert_eq!(
            registry
                .approval_sync_target_for_request("approval-sync-1")
                .unwrap()
                .unwrap()
                .operation_handle,
            operation.operation_handle
        );

        let approved = registry
            .synchronize_approval_status("approval-sync-1", "approved")
            .unwrap()
            .unwrap();
        assert_eq!(approved.state, "resume_required");
        assert_eq!(
            approved.result_code.as_deref(),
            Some("approval_resolved_resume_required")
        );
        let stale_pending = registry
            .synchronize_approval_status("approval-sync-1", "pending")
            .unwrap()
            .unwrap();
        assert_eq!(stale_pending.state, "resume_required");
        assert!(registry
            .synchronize_approval_status("approval-sync-1", "denied")
            .unwrap_err()
            .to_string()
            .contains("conflicts"));
    }

    #[test]
    fn original_redelivery_cannot_advance_a_recorded_human_approval() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let request = governed_arguments();
        let operation = pending_approval(
            &mut registry,
            "sticky-human-approval",
            crate::governed_execution::TOOL_NAME,
            &request,
            "approval-sticky-human",
        );
        registry
            .synchronize_approval_status("approval-sticky-human", "approved")
            .unwrap();

        let replay = registry
            .resolve_or_allocate(
                &codex("sticky-human-approval"),
                crate::governed_execution::TOOL_NAME,
                &request,
            )
            .unwrap();
        assert_eq!(replay.operation_handle, operation.operation_handle);
        assert_eq!(
            replay
                .recorded_result
                .as_ref()
                .and_then(|result| result.get("decision"))
                .and_then(Value::as_str),
            Some("needs_approval")
        );
        assert_eq!(
            registry
                .mark_dispatch_started(&operation.operation_handle)
                .unwrap()
                .and_then(|result| result.get("decision").cloned()),
            Some(json!("needs_approval"))
        );

        // A late duplicate response from the original submission path is also
        // replay-only. Only complete_resume may advance the approved row.
        let late_allow = registry
            .record_authorization_result(
                &operation.operation_handle,
                &json!({
                    "decision":"allow",
                    "auth_token_id":"must-not-bind",
                    "authorization_expires_at":"2099-01-01T00:00:00Z"
                }),
            )
            .unwrap();
        assert_eq!(late_allow["decision"], "needs_approval");
        let status = registry
            .durable_operation_status(&operation.operation_handle)
            .unwrap();
        assert_eq!(status.state, "resume_required");
        assert_eq!(
            status.result_code.as_deref(),
            Some("approval_resolved_resume_required")
        );
    }

    #[test]
    fn prepare_resume_rebuilds_each_origin_from_only_canonical_persisted_intent() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let direct_arguments = json!({"account_id":"account-1","amount_cents":100,"reason":"test"});
        for (call_id, tool_name, expected_origin, approval_id) in [
            (
                "resume-authorize",
                "hubu_authorize_spend",
                ResumeOrigin::AuthorizeSpend,
                "approval-authorize",
            ),
            (
                "resume-submit",
                "hubu_submit_spend",
                ResumeOrigin::SubmitSpend,
                "approval-submit",
            ),
        ] {
            let operation = pending_approval(
                &mut registry,
                call_id,
                tool_name,
                &direct_arguments,
                approval_id,
            );
            registry
                .synchronize_approval_status(approval_id, "approved")
                .unwrap();
            let ResumePreparation::Dispatch(plan) = registry
                .prepare_resume(&operation.operation_handle)
                .unwrap()
            else {
                panic!("approved operation should produce a resume plan");
            };
            assert_eq!(plan.origin, expected_origin);
            assert_eq!(plan.hubu_tool_name(), tool_name);
            assert_eq!(plan.hubu_arguments, canonicalize(&direct_arguments));
            assert_eq!(plan.operation.operation_handle, operation.operation_handle);
            assert_eq!(plan.operation.operation_key, operation.operation_key);
        }

        let governed = governed_arguments();
        let operation = pending_approval(
            &mut registry,
            "resume-governed",
            crate::governed_execution::TOOL_NAME,
            &governed,
            "approval-governed",
        );
        registry
            .synchronize_approval_status("approval-governed", "approved")
            .unwrap();
        let ResumePreparation::Dispatch(plan) = registry
            .prepare_resume(&operation.operation_handle)
            .unwrap()
        else {
            panic!("approved governed operation should produce a resume plan");
        };
        assert_eq!(plan.origin, ResumeOrigin::GovernedExecution);
        assert_eq!(plan.hubu_tool_name(), "hubu_authorize_spend");
        assert_eq!(plan.hubu_arguments, governed["authorization"]);
    }

    #[test]
    fn complete_governed_resume_binds_once_and_only_then_wakes_worker() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let request = governed_arguments();
        let operation = pending_approval(
            &mut registry,
            "complete-governed",
            crate::governed_execution::TOOL_NAME,
            &request,
            "approval-complete-governed",
        );
        registry
            .synchronize_approval_status("approval-complete-governed", "approved")
            .unwrap();
        let ResumePreparation::Dispatch(plan) = registry
            .prepare_resume(&operation.operation_handle)
            .unwrap()
        else {
            panic!("approved governed operation should produce a resume plan");
        };
        let result = json!({
            "decision":"allow",
            "decision_id":"approval-complete-governed",
            "auth_token_id":"resume-token-1",
            "authorization_expires_at":"2099-01-01T00:00:00Z",
            "operation_handle":operation.operation_handle
        });
        let completion = registry.complete_resume(&plan, &result).unwrap();
        assert_eq!(completion.authoritative_result, result);
        assert_eq!(completion.status.state, "accepted");
        assert!(completion.wake_operation_worker);

        let stored: (Option<String>, Option<String>, Option<String>) = registry
            .connection
            .query_row(
                "SELECT normalized_request_json, gongbu_request_hash, gongbu_request_json
                 FROM harness_operations WHERE operation_handle = ?1",
                [&operation.operation_handle],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert!(stored.0.is_none());
        assert!(stored.1.is_some());
        let gongbu_request: Value = serde_json::from_str(stored.2.as_deref().unwrap()).unwrap();
        assert_eq!(gongbu_request["spend_auth_token_id"], "resume-token-1");
        let ResumePreparation::Replay {
            origin,
            authoritative_result,
            status,
        } = registry
            .prepare_resume(&operation.operation_handle)
            .unwrap()
        else {
            panic!("completed governed resume should replay without backend access");
        };
        assert_eq!(origin, ResumeOrigin::GovernedExecution);
        assert_eq!(authoritative_result, result);
        assert_eq!(status.state, "accepted");

        let replay = registry.complete_resume(&plan, &result).unwrap();
        assert_eq!(replay.status.state, "accepted");
        assert!(!replay.wake_operation_worker);
        assert_eq!(registry.promote_accepted_operations().unwrap(), 1);
        assert!(registry.claim_due_operation().unwrap().is_some());
        assert!(registry.claim_due_operation().unwrap().is_none());
    }

    #[test]
    fn expired_governed_resume_is_terminal_without_binding_execution() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let request = governed_arguments();
        let operation = pending_approval(
            &mut registry,
            "expired-governed-resume",
            crate::governed_execution::TOOL_NAME,
            &request,
            "approval-expired-governed",
        );
        registry
            .synchronize_approval_status("approval-expired-governed", "approved")
            .unwrap();
        let ResumePreparation::Dispatch(plan) = registry
            .prepare_resume(&operation.operation_handle)
            .unwrap()
        else {
            panic!("approved governed operation should produce a resume plan");
        };

        let completion = registry
            .complete_resume(
                &plan,
                &json!({
                    "decision":"allow",
                    "decision_id":"approval-expired-governed",
                    "auth_token_id":"expired-resume-token",
                    "authorization_expires_at":"2000-01-01T00:00:00Z",
                    "operation_handle":operation.operation_handle
                }),
            )
            .unwrap();
        assert_eq!(completion.status.state, "failed");
        assert_eq!(
            completion.status.result_code.as_deref(),
            Some("authorization_expired_before_resume")
        );
        assert!(!completion.wake_operation_worker);
        let stored: (Option<String>, Option<String>, Option<String>) = registry
            .connection
            .query_row(
                "SELECT normalized_request_json, gongbu_request_hash, gongbu_request_json
                 FROM harness_operations WHERE operation_handle = ?1",
                [&operation.operation_handle],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(stored, (None, None, None));
        assert!(matches!(
            registry
                .prepare_resume(&operation.operation_handle)
                .unwrap(),
            ResumePreparation::Status(_)
        ));
    }

    #[test]
    fn expired_authorize_resume_clears_the_continuation_and_is_replacement_safe() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let request = json!({"account_id":"account-1","amount_cents":100,"reason":"test"});
        let operation = pending_approval(
            &mut registry,
            "expired-authorize-resume",
            "hubu_authorize_spend",
            &request,
            "approval-expired-authorize",
        );
        registry
            .synchronize_approval_status("approval-expired-authorize", "approved")
            .unwrap();
        let ResumePreparation::Dispatch(plan) = registry
            .prepare_resume(&operation.operation_handle)
            .unwrap()
        else {
            panic!("approved authorization should produce a resume plan");
        };
        assert_eq!(plan.origin, ResumeOrigin::AuthorizeSpend);

        let completion = registry
            .complete_resume(
                &plan,
                &json!({
                    "decision":"allow",
                    "decision_id":"approval-expired-authorize",
                    "auth_token_id":"expired-direct-token",
                    "authorization_expires_at":"2000-01-01T00:00:00Z",
                    "retry_guidance":{
                        "action":"replay_exactly",
                        "message":"replay the expired authorization"
                    },
                    "operation_handle":operation.operation_handle
                }),
            )
            .unwrap();
        assert_eq!(completion.status.state, "failed");
        assert_eq!(
            completion.status.result_code.as_deref(),
            Some("authorization_expired_before_resume")
        );
        assert!(!completion.wake_operation_worker);
        assert!(completion
            .authoritative_result
            .get("auth_token_id")
            .is_none());
        assert_eq!(
            completion.authoritative_result["retry_guidance"]["action"],
            "create_new_operation"
        );

        let stored: (Option<String>, Option<String>, String) = registry
            .connection
            .query_row(
                "SELECT normalized_request_json, auth_token_id, result_json
                 FROM harness_operations WHERE operation_handle = ?1",
                [&operation.operation_handle],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert!(stored.0.is_none());
        assert!(stored.1.is_none());
        let stored_result: Value = serde_json::from_str(&stored.2).unwrap();
        assert!(stored_result.get("auth_token_id").is_none());
        assert_eq!(
            stored_result["retry_guidance"]["action"],
            "create_new_operation"
        );
        assert!(matches!(
            registry
                .prepare_resume(&operation.operation_handle)
                .unwrap(),
            ResumePreparation::Status(status)
                if status.terminal()
                    && status.result_code.as_deref()
                        == Some("authorization_expired_before_resume")
        ));
    }

    #[test]
    fn malformed_governed_resume_is_terminal_without_waking_worker() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let request = governed_arguments();
        let operation = pending_approval(
            &mut registry,
            "malformed-governed-resume",
            crate::governed_execution::TOOL_NAME,
            &request,
            "approval-malformed-governed",
        );
        registry
            .synchronize_approval_status("approval-malformed-governed", "approved")
            .unwrap();
        let ResumePreparation::Dispatch(plan) = registry
            .prepare_resume(&operation.operation_handle)
            .unwrap()
        else {
            panic!("approved governed operation should produce a resume plan");
        };
        let malformed = json!({
            "decision":"allow",
            "decision_id":"approval-malformed-governed",
            "authorization_expires_at":"2099-01-01T00:00:00Z"
        });

        let completion = registry.complete_resume(&plan, &malformed).unwrap();
        assert_eq!(completion.status.state, "failed");
        assert_eq!(
            completion.status.result_code.as_deref(),
            Some("authorization_continuation_unavailable")
        );
        assert!(!completion.wake_operation_worker);
        let stored: (Option<String>, Option<String>) = registry
            .connection
            .query_row(
                "SELECT normalized_request_json, gongbu_request_json
                 FROM harness_operations WHERE operation_handle = ?1",
                [&operation.operation_handle],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(stored, (None, None));
        assert!(matches!(
            registry
                .prepare_resume(&operation.operation_handle)
                .unwrap(),
            ResumePreparation::Status(_)
        ));
    }

    #[test]
    fn direct_resume_only_becomes_terminal_after_exact_route_result() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let arguments = json!({"account_id":"account-1","amount_cents":100,"reason":"test"});
        for (call_id, tool_name, approval_id, expected_state, result) in [
            (
                "complete-authorize",
                "hubu_authorize_spend",
                "approval-complete-authorize",
                "authorized",
                json!({
                    "decision":"allow",
                    "auth_token_id":"direct-authorization-token",
                    "authorization_expires_at":"2099-01-01T00:00:00Z"
                }),
            ),
            (
                "complete-submit",
                "hubu_submit_spend",
                "approval-complete-submit",
                "succeeded",
                json!({
                    "decision":"allow",
                    "auth_token_id":"direct-submit-token",
                    "requires_human_approval":false,
                    "payment":{
                        "payment_id":"payment-complete-submit",
                        "status":"succeeded"
                    }
                }),
            ),
        ] {
            let operation =
                pending_approval(&mut registry, call_id, tool_name, &arguments, approval_id);
            let synced = registry
                .synchronize_approval_status(approval_id, "approved")
                .unwrap()
                .unwrap();
            assert_eq!(synced.state, "resume_required");
            let ResumePreparation::Dispatch(plan) = registry
                .prepare_resume(&operation.operation_handle)
                .unwrap()
            else {
                panic!("approved direct operation should produce a resume plan");
            };
            let completion = registry.complete_resume(&plan, &result).unwrap();
            assert_eq!(completion.status.state, expected_state);
            assert!(!completion.wake_operation_worker);
            let ResumePreparation::Replay {
                origin,
                authoritative_result,
                status,
            } = registry
                .prepare_resume(&operation.operation_handle)
                .unwrap()
            else {
                panic!("completed direct resume should replay without backend access");
            };
            assert_eq!(origin.hubu_tool_name(), tool_name);
            assert_eq!(authoritative_result, result);
            assert_eq!(status.state, expected_state);
        }
    }

    #[test]
    fn successful_submit_resume_canonicalizes_legacy_approval_decision() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let arguments = json!({"account_id":"account-1","amount_cents":100,"reason":"test"});
        let operation = pending_approval(
            &mut registry,
            "legacy-approved-submit",
            "hubu_submit_spend",
            &arguments,
            "approval-legacy-approved-submit",
        );
        registry
            .synchronize_approval_status("approval-legacy-approved-submit", "approved")
            .unwrap();
        let ResumePreparation::Dispatch(plan) = registry
            .prepare_resume(&operation.operation_handle)
            .unwrap()
        else {
            panic!("approved submit should produce a resume plan");
        };
        let completion = registry
            .complete_resume(
                &plan,
                &json!({
                    "decision":"needs_approval",
                    "requires_human_approval":true,
                    "approval_reason":"payment did not execute",
                    "retry_guidance":{
                        "action":"replay_exactly",
                        "message":"replay after approval"
                    },
                    "payment":{
                        "payment_id":"payment-legacy-approved-submit",
                        "status":"succeeded"
                    }
                }),
            )
            .unwrap();

        assert_eq!(completion.status.state, "succeeded");
        assert_eq!(
            completion.status.result_code.as_deref(),
            Some("spend_succeeded")
        );
        assert_eq!(completion.authoritative_result["decision"], "allow");
        assert_eq!(
            completion.authoritative_result["requires_human_approval"],
            false
        );
        assert!(completion
            .authoritative_result
            .get("approval_reason")
            .is_none());
        assert!(completion
            .authoritative_result
            .get("retry_guidance")
            .is_none());

        let stored: (String, bool, String, String, String) = registry
            .connection
            .query_row(
                "SELECT decision, normalized_request_json IS NULL, result_json,
                        operation_state, operation_result_code
                 FROM harness_operations WHERE operation_handle = ?1",
                [&operation.operation_handle],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(stored.0, "allow");
        assert!(stored.1);
        let stored_result: Value = serde_json::from_str(&stored.2).unwrap();
        assert_eq!(stored_result, completion.authoritative_result);
        assert_eq!(stored.3, "succeeded");
        assert_eq!(stored.4, "spend_succeeded");

        let ResumePreparation::Replay {
            origin,
            authoritative_result,
            status,
        } = registry
            .prepare_resume(&operation.operation_handle)
            .unwrap()
        else {
            panic!("completed submit should replay without backend access");
        };
        assert_eq!(origin, ResumeOrigin::SubmitSpend);
        assert_eq!(authoritative_result, completion.authoritative_result);
        assert_eq!(status.state, "succeeded");
    }

    #[test]
    fn failed_submit_resume_canonicalizes_legacy_approval_decision_and_replays() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let arguments = json!({"account_id":"account-1","amount_cents":100,"reason":"test"});
        let operation = pending_approval(
            &mut registry,
            "legacy-failed-submit",
            "hubu_submit_spend",
            &arguments,
            "approval-legacy-failed-submit",
        );
        registry
            .synchronize_approval_status("approval-legacy-failed-submit", "approved")
            .unwrap();
        let ResumePreparation::Dispatch(plan) = registry
            .prepare_resume(&operation.operation_handle)
            .unwrap()
        else {
            panic!("approved submit should produce a resume plan");
        };
        let completion = registry
            .complete_resume(
                &plan,
                &json!({
                    "decision":"needs_approval",
                    "requires_human_approval":true,
                    "approval_reason":"payment outcome needs review",
                    "retry_guidance":{
                        "action":"replay_exactly",
                        "message":"replay after approval"
                    },
                    "payment":{
                        "payment_id":"payment-legacy-failed-submit",
                        "status":"failed",
                        "failure_reason":"rail declined"
                    }
                }),
            )
            .unwrap();

        assert_eq!(completion.status.state, "failed");
        assert_eq!(
            completion.status.result_code.as_deref(),
            Some("spend_failed")
        );
        assert_eq!(completion.authoritative_result["decision"], "allow");
        assert_eq!(
            completion.authoritative_result["requires_human_approval"],
            false
        );
        assert!(completion
            .authoritative_result
            .get("approval_reason")
            .is_none());
        assert!(completion
            .authoritative_result
            .get("retry_guidance")
            .is_none());

        let stored: (String, bool, String, String, String) = registry
            .connection
            .query_row(
                "SELECT decision, normalized_request_json IS NULL, result_json,
                        operation_state, operation_result_code
                 FROM harness_operations WHERE operation_handle = ?1",
                [&operation.operation_handle],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(stored.0, "allow");
        assert!(stored.1);
        let stored_result: Value = serde_json::from_str(&stored.2).unwrap();
        assert_eq!(stored_result, completion.authoritative_result);
        assert_eq!(stored.3, "failed");
        assert_eq!(stored.4, "spend_failed");

        let ResumePreparation::Replay {
            origin,
            authoritative_result,
            status,
        } = registry
            .prepare_resume(&operation.operation_handle)
            .unwrap()
        else {
            panic!("failed submit receipt should replay without backend access");
        };
        assert_eq!(origin, ResumeOrigin::SubmitSpend);
        assert_eq!(authoritative_result, completion.authoritative_result);
        assert_eq!(status.state, "failed");
        assert_eq!(status.result_code.as_deref(), Some("spend_failed"));
    }

    #[test]
    fn late_terminal_submit_receipt_supersedes_only_pre_execution_expiry() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let arguments = json!({"account_id":"account-1","amount_cents":100,"reason":"test"});
        let operation = pending_approval(
            &mut registry,
            "late-success-after-expiry",
            "hubu_submit_spend",
            &arguments,
            "approval-late-success-after-expiry",
        );
        registry
            .synchronize_approval_status("approval-late-success-after-expiry", "approved")
            .unwrap();
        let ResumePreparation::Dispatch(plan) = registry
            .prepare_resume(&operation.operation_handle)
            .unwrap()
        else {
            panic!("approved submit should produce a resume plan");
        };

        let expired = registry
            .fail_pre_execution_operation(
                &operation.operation_handle,
                "authorization_expired_before_resume",
            )
            .unwrap();
        assert_eq!(expired.state, "failed");
        assert_eq!(
            expired.result_code.as_deref(),
            Some("authorization_expired_before_resume")
        );

        let completion = registry
            .complete_resume(
                &plan,
                &json!({
                    "decision":"needs_approval",
                    "requires_human_approval":true,
                    "retry_guidance":{
                        "action":"create_new_operation",
                        "message":"stale expiry guidance"
                    },
                    "payment":{
                        "payment_id":"payment-late-success-after-expiry",
                        "status":"succeeded"
                    }
                }),
            )
            .unwrap();
        assert_eq!(completion.status.state, "succeeded");
        assert_eq!(
            completion.status.result_code.as_deref(),
            Some("spend_succeeded")
        );
        assert_eq!(completion.authoritative_result["decision"], "allow");
        assert_eq!(
            completion.authoritative_result["requires_human_approval"],
            false
        );
        assert!(completion
            .authoritative_result
            .get("retry_guidance")
            .is_none());

        let stored: (Option<String>, String, String, String) = registry
            .connection
            .query_row(
                "SELECT normalized_request_json, operation_state,
                        operation_result_code, result_json
                 FROM harness_operations WHERE operation_handle = ?1",
                [&operation.operation_handle],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert!(stored.0.is_none());
        assert_eq!(stored.1, "succeeded");
        assert_eq!(stored.2, "spend_succeeded");
        assert_eq!(
            serde_json::from_str::<Value>(&stored.3).unwrap(),
            completion.authoritative_result
        );

        let ResumePreparation::Replay {
            origin,
            authoritative_result,
            status,
        } = registry
            .prepare_resume(&operation.operation_handle)
            .unwrap()
        else {
            panic!("late terminal receipt should become the durable replay result");
        };
        assert_eq!(origin, ResumeOrigin::SubmitSpend);
        assert_eq!(authoritative_result, completion.authoritative_result);
        assert_eq!(status.state, "succeeded");
        assert_eq!(status.result_code.as_deref(), Some("spend_succeeded"));
    }

    #[test]
    fn submit_resume_rejects_allow_without_valid_terminal_payment_receipt() {
        let arguments = json!({"account_id":"account-1","amount_cents":100,"reason":"test"});
        for (suffix, payment) in [
            ("malformed", json!("not-an-object")),
            (
                "nonterminal",
                json!({"payment_id":"payment-pending","status":"pending"}),
            ),
            ("missing-id", json!({"status":"succeeded"})),
            ("empty-id", json!({"payment_id":"","status":"failed"})),
        ] {
            let mut registry = OperationRegistry::open_in_memory().unwrap();
            let approval_id = format!("approval-invalid-submit-{suffix}");
            let operation = pending_approval(
                &mut registry,
                &format!("invalid-submit-{suffix}"),
                "hubu_submit_spend",
                &arguments,
                &approval_id,
            );
            registry
                .synchronize_approval_status(&approval_id, "approved")
                .unwrap();
            let ResumePreparation::Dispatch(plan) = registry
                .prepare_resume(&operation.operation_handle)
                .unwrap()
            else {
                panic!("approved submit should produce a resume plan");
            };

            let error = registry
                .complete_resume(
                    &plan,
                    &json!({
                        "decision":"allow",
                        "payment":payment
                    }),
                )
                .unwrap_err();
            assert!(error
                .to_string()
                .contains("missing a valid terminal payment receipt"));
            let status = registry
                .durable_operation_status(&operation.operation_handle)
                .unwrap();
            assert_eq!(status.state, "resume_required");
            let stored: (String, bool) = registry
                .connection
                .query_row(
                    "SELECT decision, normalized_request_json IS NOT NULL
                     FROM harness_operations WHERE operation_handle = ?1",
                    [&operation.operation_handle],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(stored, ("needs_approval".into(), true));
        }
    }

    #[test]
    fn submit_resume_requires_approved_status_and_a_consistent_terminal_decision() {
        let mut registry = OperationRegistry::open_in_memory().unwrap();
        let arguments = json!({"account_id":"account-1","amount_cents":100,"reason":"test"});
        let operation = pending_approval(
            &mut registry,
            "submit-success-guards",
            "hubu_submit_spend",
            &arguments,
            "approval-submit-success-guards",
        );
        let plan = ResumeOperationPlan {
            origin: ResumeOrigin::SubmitSpend,
            operation: operation.clone(),
            hubu_arguments: arguments,
        };
        let payment = json!({
            "payment_id":"payment-submit-success-guards",
            "status":"succeeded"
        });

        let unapproved = registry
            .complete_resume(
                &plan,
                &json!({"decision":"needs_approval","payment":payment}),
            )
            .unwrap_err();
        assert!(unapproved
            .to_string()
            .contains("payment is missing durable approval"));
        assert_eq!(
            registry
                .durable_operation_status(&operation.operation_handle)
                .unwrap()
                .state,
            "approval_required"
        );

        registry
            .synchronize_approval_status("approval-submit-success-guards", "approved")
            .unwrap();
        let inconsistent = registry
            .complete_resume(
                &plan,
                &json!({
                    "decision":"deny",
                    "payment":{
                        "payment_id":"payment-submit-success-guards",
                        "status":"succeeded"
                    }
                }),
            )
            .unwrap_err();
        assert!(inconsistent
            .to_string()
            .contains("conflicts with its terminal payment receipt"));
        let inconsistent_failure = registry
            .complete_resume(
                &plan,
                &json!({
                    "decision":"deny",
                    "payment":{
                        "payment_id":"payment-submit-failure-guards",
                        "status":"failed"
                    }
                }),
            )
            .unwrap_err();
        assert!(inconsistent_failure
            .to_string()
            .contains("conflicts with its terminal payment receipt"));
        assert_eq!(
            registry
                .durable_operation_status(&operation.operation_handle)
                .unwrap()
                .state,
            "resume_required"
        );
    }

    #[test]
    fn submit_resume_does_not_canonicalize_pending_or_denied_without_payment() {
        let arguments = json!({"account_id":"account-1","amount_cents":100,"reason":"test"});

        let mut pending_registry = OperationRegistry::open_in_memory().unwrap();
        let pending = pending_approval(
            &mut pending_registry,
            "still-pending-submit",
            "hubu_submit_spend",
            &arguments,
            "approval-still-pending-submit",
        );
        pending_registry
            .synchronize_approval_status("approval-still-pending-submit", "approved")
            .unwrap();
        let ResumePreparation::Dispatch(pending_plan) = pending_registry
            .prepare_resume(&pending.operation_handle)
            .unwrap()
        else {
            panic!("approved submit should produce a resume plan");
        };
        let completion = pending_registry
            .complete_resume(
                &pending_plan,
                &json!({"decision":"needs_approval","payment":null}),
            )
            .unwrap();
        assert_eq!(
            completion.authoritative_result["decision"],
            "needs_approval"
        );
        assert_eq!(completion.status.state, "resume_required");
        let intent_retained: bool = pending_registry
            .connection
            .query_row(
                "SELECT normalized_request_json IS NOT NULL
                 FROM harness_operations WHERE operation_handle = ?1",
                [&pending.operation_handle],
                |row| row.get(0),
            )
            .unwrap();
        assert!(intent_retained);

        let mut denied_registry = OperationRegistry::open_in_memory().unwrap();
        let denied = pending_approval(
            &mut denied_registry,
            "denied-submit-result",
            "hubu_submit_spend",
            &arguments,
            "approval-denied-submit-result",
        );
        denied_registry
            .synchronize_approval_status("approval-denied-submit-result", "approved")
            .unwrap();
        let ResumePreparation::Dispatch(denied_plan) = denied_registry
            .prepare_resume(&denied.operation_handle)
            .unwrap()
        else {
            panic!("approved submit should produce a resume plan");
        };
        assert!(denied_registry
            .complete_resume(&denied_plan, &json!({"decision":"deny","payment":null}))
            .unwrap_err()
            .to_string()
            .contains("approval status conflicts"));
        assert_eq!(
            denied_registry
                .durable_operation_status(&denied.operation_handle)
                .unwrap()
                .state,
            "resume_required"
        );
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
        assert_eq!(
            recovered_pending
                .recorded_result
                .as_ref()
                .and_then(|result| result.get("decision")),
            Some(&json!("needs_approval"))
        );

        registry
            .synchronize_approval_status("approval-1", "approved")
            .unwrap();
        let ResumePreparation::Dispatch(plan) =
            registry.prepare_resume(&pending.operation_handle).unwrap()
        else {
            panic!("approved operation should resume only by public handle");
        };

        let terminal = json!({
            "decision":"allow",
            "decision_id":"decision-1",
            "auth_token_id":"authorization-1",
            "authorization_expires_at":"2099-01-01T00:00:00Z",
            "operation_handle":pending.operation_handle
        });
        registry.complete_resume(&plan, &terminal).unwrap();
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
                Some("approval-1".into())
            )
        );
        let normalized_request: Option<String> = registry
            .connection
            .query_row(
                "SELECT normalized_request_json FROM harness_operations
                 WHERE operation_handle = ?1",
                [&pending.operation_handle],
                |row| row.get(0),
            )
            .unwrap();
        assert!(normalized_request.is_none());
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
    fn v4_registry_migrates_forward_and_missing_resume_intent_fails_terminally() {
        let root = tempdir().unwrap();
        let path = root.path().join("operations-v4.sqlite3");
        let arguments = governed_arguments();
        let mut registry = OperationRegistry::open(&path).unwrap();
        let pending = pending_approval(
            &mut registry,
            "migration-v4-pending",
            crate::governed_execution::TOOL_NAME,
            &arguments,
            "approval-migration-v4",
        );
        let recoverable = pending_approval(
            &mut registry,
            "migration-v4-exact-redelivery",
            crate::governed_execution::TOOL_NAME,
            &arguments,
            "approval-migration-v4-recoverable",
        );
        drop(registry);

        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "DROP INDEX harness_operation_key_record;
                 DROP INDEX harness_operation_approval_request;
                 ALTER TABLE harness_operations DROP COLUMN operation_key_record_id;
                 ALTER TABLE harness_operations DROP COLUMN governed_result_json;
                 ALTER TABLE harness_operations DROP COLUMN normalized_request_json;
                 ALTER TABLE harness_operations DROP COLUMN approval_status;
                 ALTER TABLE harness_operations DROP COLUMN approval_synced_at;
                 PRAGMA user_version = 4;",
            )
            .unwrap();
        drop(connection);

        let mut migrated = OperationRegistry::open(&path).unwrap();
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
        drop(connection);
        let recovered_before_resume = migrated
            .resolve_or_allocate(
                &codex("migration-v4-exact-redelivery"),
                crate::governed_execution::TOOL_NAME,
                &arguments,
            )
            .unwrap();
        assert_eq!(
            recovered_before_resume.operation_handle,
            recoverable.operation_handle
        );
        migrated
            .synchronize_approval_status("approval-migration-v4-recoverable", "approved")
            .unwrap();
        assert!(matches!(
            migrated
                .prepare_resume(&recoverable.operation_handle)
                .unwrap(),
            ResumePreparation::Dispatch(_)
        ));

        let approved = migrated
            .synchronize_approval_status("approval-migration-v4", "approved")
            .unwrap()
            .unwrap();
        assert_eq!(approved.state, "resume_required");
        assert_eq!(
            approved.result_code.as_deref(),
            Some("resume_intent_unavailable")
        );
        let ResumePreparation::IntentUnavailable(failed) =
            migrated.prepare_resume(&pending.operation_handle).unwrap()
        else {
            panic!("legacy approval without intent must fail deterministically");
        };
        assert_eq!(failed.state, "failed");
        assert_eq!(
            failed.result_code.as_deref(),
            Some("resume_intent_unavailable")
        );
        assert!(failed.terminal());

        let recovered = migrated
            .resolve_or_allocate(
                &codex("migration-v4-pending"),
                crate::governed_execution::TOOL_NAME,
                &arguments,
            )
            .unwrap();
        assert_eq!(recovered.operation_handle, pending.operation_handle);
        assert!(matches!(
            migrated.prepare_resume(&pending.operation_handle).unwrap(),
            ResumePreparation::Status(status) if status.state == "failed"
        ));
        let normalized_request: Option<String> = migrated
            .connection
            .query_row(
                "SELECT normalized_request_json FROM harness_operations
                 WHERE operation_handle = ?1",
                [&pending.operation_handle],
                |row| row.get(0),
            )
            .unwrap();
        assert!(normalized_request.is_none());
    }

    #[test]
    fn v1_registry_is_rejected_for_the_v6_forward_migration_contract() {
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
