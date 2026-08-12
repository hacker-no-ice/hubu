use std::{fs, path::Path, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

pub const TRUSTED_INVOCATION_META_KEY: &str = "hubu.dev/platform-invocation";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TrustedInvocationIdentity {
    pub platform: String,
    pub installation_id: String,
    pub invocation_id: String,
}

impl TrustedInvocationIdentity {
    pub fn from_call_params(params: &Value) -> Result<Self> {
        let metadata = params
            .get("_meta")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                anyhow!(
                    "Hubu spend tools require trusted params._meta.{TRUSTED_INVOCATION_META_KEY} metadata supplied by the MCP client"
                )
            })?;
        let identity = metadata
            .get(TRUSTED_INVOCATION_META_KEY)
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "Hubu spend tools require trusted params._meta.{TRUSTED_INVOCATION_META_KEY} metadata supplied by the MCP client"
                )
            })?;
        let identity: Self = serde_json::from_value(identity)
            .context("trusted Hubu platform invocation metadata is invalid")?;
        identity.validate()?;
        Ok(identity)
    }

    fn validate(&self) -> Result<()> {
        validate_platform(&self.platform)?;
        validate_opaque_id("installation_id", &self.installation_id)?;
        validate_opaque_id("invocation_id", &self.invocation_id)?;
        Ok(())
    }
}

fn validate_platform(platform: &str) -> Result<()> {
    if platform.is_empty() || platform.len() > 64 {
        bail!("trusted invocation platform must contain 1 to 64 characters");
    }
    if !platform
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        bail!(
            "trusted invocation platform may contain only ASCII letters, numbers, '.', '_', and '-'"
        );
    }
    Ok(())
}

fn validate_opaque_id(field: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 512 {
        bail!("trusted invocation {field} must contain 1 to 512 characters");
    }
    if value.trim() != value || value.chars().any(char::is_control) {
        bail!("trusted invocation {field} cannot contain surrounding whitespace or control characters");
    }
    Ok(())
}

pub struct OperationKeyRegistry {
    connection: Connection,
}

impl OperationKeyRegistry {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "create Hubu MCP operation-key state directory `{}`",
                    parent.display()
                )
            })?;
        }
        let connection = Connection::open(path)
            .with_context(|| format!("open Hubu MCP operation-key state `{}`", path.display()))?;
        Self::from_connection(connection)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self> {
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS trusted_platform_invocations (
                 platform TEXT NOT NULL,
                 installation_id TEXT NOT NULL,
                 invocation_id TEXT NOT NULL,
                 operation_key TEXT NOT NULL UNIQUE,
                 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                 PRIMARY KEY(platform, installation_id, invocation_id)
             );",
        )?;
        Ok(Self { connection })
    }

    pub fn resolve_or_allocate(&mut self, identity: &TrustedInvocationIdentity) -> Result<String> {
        identity.validate()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(operation_key) = transaction
            .query_row(
                "SELECT operation_key
                 FROM trusted_platform_invocations
                 WHERE platform = ?1 AND installation_id = ?2 AND invocation_id = ?3",
                params![
                    identity.platform,
                    identity.installation_id,
                    identity.invocation_id
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            transaction.commit()?;
            return Ok(operation_key);
        }

        let operation_key = format!("hubu:v1:{}:{}", identity.platform, Uuid::new_v4().simple());
        transaction.execute(
            "INSERT INTO trusted_platform_invocations
             (platform, installation_id, invocation_id, operation_key)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                identity.platform,
                identity.installation_id,
                identity.invocation_id,
                operation_key
            ],
        )?;
        transaction.commit()?;
        Ok(operation_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(invocation_id: &str) -> TrustedInvocationIdentity {
        TrustedInvocationIdentity {
            platform: "codex".to_string(),
            installation_id: "installation-1".to_string(),
            invocation_id: invocation_id.to_string(),
        }
    }

    #[test]
    fn retry_reuses_the_same_namespaced_operation_key() {
        let mut registry = OperationKeyRegistry::open_in_memory().unwrap();
        let first = registry.resolve_or_allocate(&identity("call-1")).unwrap();
        let retry = registry.resolve_or_allocate(&identity("call-1")).unwrap();

        assert_eq!(retry, first);
        assert!(first.starts_with("hubu:v1:codex:"));
    }

    #[test]
    fn separate_provider_invocations_receive_distinct_keys() {
        let mut registry = OperationKeyRegistry::open_in_memory().unwrap();
        let first = registry
            .resolve_or_allocate(&identity("provider-a"))
            .unwrap();
        let second = registry
            .resolve_or_allocate(&identity("provider-b"))
            .unwrap();

        assert_ne!(second, first);
    }

    #[test]
    fn process_restart_recovers_the_same_operation_key() {
        let path = std::env::temp_dir().join(format!(
            "hubu-mcp-operation-keys-{}.sqlite3",
            Uuid::new_v4()
        ));
        let first = {
            let mut registry = OperationKeyRegistry::open(&path).unwrap();
            registry
                .resolve_or_allocate(&identity("recovered-call"))
                .unwrap()
        };
        let recovered = {
            let mut registry = OperationKeyRegistry::open(&path).unwrap();
            registry
                .resolve_or_allocate(&identity("recovered-call"))
                .unwrap()
        };

        assert_eq!(recovered, first);
        std::fs::remove_file(path).ok();
    }
}
