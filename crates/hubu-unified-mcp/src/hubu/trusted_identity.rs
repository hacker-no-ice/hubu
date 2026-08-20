use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;

pub const TRUSTED_INVOCATION_META_KEY: &str = "hubu.dev/platform-invocation";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TrustedSpendIdentity {
    pub platform: String,
    pub installation_id: String,
    pub invocation_id: String,
    pub operation_key: String,
    #[serde(default)]
    pub task_id: Option<String>,
}

impl TrustedSpendIdentity {
    pub fn from_call_params(params: &Value) -> Result<Self> {
        let identity = params
            .get("_meta")
            .and_then(Value::as_object)
            .and_then(|metadata| metadata.get(TRUSTED_INVOCATION_META_KEY))
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
        if self.platform.is_empty()
            || self.platform.len() > 64
            || !self
                .platform
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            bail!("trusted invocation platform is invalid");
        }
        validate_opaque_id("installation_id", &self.installation_id)?;
        validate_opaque_id("invocation_id", &self.invocation_id)?;
        validate_opaque_id("operation_key", &self.operation_key)?;
        if let Some(task_id) = &self.task_id {
            validate_opaque_id("task_id", task_id)?;
        }
        Ok(())
    }
}

fn validate_opaque_id(field: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 512
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        bail!("trusted invocation {field} is invalid");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_complete_trusted_identity() {
        let identity = TrustedSpendIdentity::from_call_params(&json!({
            "_meta": {TRUSTED_INVOCATION_META_KEY: {
                "platform": "codex",
                "installation_id": "installation-1",
                "invocation_id": "call-1",
                "operation_key": "platform:op-1",
                "task_id": "linear:HUB-73"
            }}
        }))
        .unwrap();
        assert_eq!(identity.operation_key, "platform:op-1");
        assert_eq!(identity.task_id.as_deref(), Some("linear:HUB-73"));
    }
}
