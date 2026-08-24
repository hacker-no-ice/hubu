use std::{fs, path::Path, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
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
const SCHEMA_VERSION: i64 = 1;

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
    pub(crate) operation_key: String,
    pub(crate) task_id: Option<String>,
}

#[derive(Debug)]
struct PersistedOperation {
    request_hash: String,
    operation_key: String,
    codex_call_id: Option<String>,
    claude_tool_use_id: Option<String>,
    hubu_invocation_id: Option<String>,
    task_id: Option<String>,
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
        if path != Path::new(":memory:") {
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
        }

        let existed = path.exists();
        let connection = Connection::open(path)
            .with_context(|| format!("open unified MCP operation registry `{}`", path.display()))?;
        #[cfg(unix)]
        if !existed && path != Path::new(":memory:") {
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
        let version =
            connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?;
        if version != 0 && version != SCHEMA_VERSION {
            bail!("unified MCP operation registry schema version {version} is unsupported; expected {SCHEMA_VERSION}");
        }
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS installation_identity (
                 singleton INTEGER NOT NULL PRIMARY KEY CHECK(singleton = 1),
                 installation_id TEXT NOT NULL UNIQUE CHECK(length(installation_id) BETWEEN 1 AND 128),
                 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
             );
             CREATE TABLE IF NOT EXISTS harness_operations (
                 platform TEXT NOT NULL CHECK(length(platform) BETWEEN 1 AND 64),
                 installation_id TEXT NOT NULL CHECK(length(installation_id) BETWEEN 1 AND 128),
                 harness_call_id TEXT NOT NULL CHECK(length(harness_call_id) BETWEEN 1 AND 512),
                 request_hash TEXT NOT NULL CHECK(length(request_hash) = 71),
                 operation_key TEXT NOT NULL UNIQUE CHECK(length(operation_key) BETWEEN 1 AND 160),
                 codex_call_id TEXT CHECK(codex_call_id IS NULL OR length(codex_call_id) BETWEEN 1 AND 512),
                 claude_tool_use_id TEXT CHECK(claude_tool_use_id IS NULL OR length(claude_tool_use_id) BETWEEN 1 AND 512),
                 hubu_invocation_id TEXT CHECK(hubu_invocation_id IS NULL OR length(hubu_invocation_id) BETWEEN 1 AND 512),
                 controlled_installation_id TEXT CHECK(controlled_installation_id IS NULL OR length(controlled_installation_id) BETWEEN 1 AND 512),
                 task_id TEXT CHECK(task_id IS NULL OR length(task_id) BETWEEN 1 AND 512),
                 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                 PRIMARY KEY(platform, installation_id, harness_call_id),
                 FOREIGN KEY(installation_id) REFERENCES installation_identity(installation_id)
             );
             PRAGMA user_version = 1;",
        )?;
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
        Ok(Self {
            connection,
            installation_id,
        })
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
                "SELECT request_hash, operation_key, codex_call_id, claude_tool_use_id,
                        hubu_invocation_id, task_id
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
                        codex_call_id: row.get(2)?,
                        claude_tool_use_id: row.get(3)?,
                        hubu_invocation_id: row.get(4)?,
                        task_id: row.get(5)?,
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
            return Ok(OperationResolution {
                operation_key: existing.operation_key,
                task_id: existing.task_id,
            });
        }

        let operation_key = format!(
            "hubu:operation:v1:{}:{}",
            identity.platform,
            Uuid::new_v4().simple()
        );
        transaction.execute(
            "INSERT INTO harness_operations (
                 platform, installation_id, harness_call_id, request_hash, operation_key,
                 codex_call_id, claude_tool_use_id, hubu_invocation_id,
                 controlled_installation_id, task_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                identity.platform,
                self.installation_id,
                identity.harness_call_id,
                request_hash,
                operation_key,
                identity.codex_call_id,
                identity.claude_tool_use_id,
                identity.hubu_invocation_id,
                identity.controlled_installation_id,
                identity.task_id,
            ],
        )?;
        transaction.commit()?;
        Ok(OperationResolution {
            operation_key,
            task_id: identity.task_id.clone(),
        })
    }
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
        assert!(first.operation_key.starts_with("hubu:operation:v1:codex:"));

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
