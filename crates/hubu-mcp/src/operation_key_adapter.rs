use std::env;

use anyhow::{anyhow, bail, Result};
use serde_json::Value;

pub(crate) const PLATFORM_NAMESPACE_ENV: &str = "HUBU_MCP_PLATFORM_NAMESPACE";
pub(crate) const OPERATION_METADATA_KEY: &str = "io.hubu/operation";

const MAX_NAMESPACE_LEN: usize = 48;
const MAX_OPERATION_ID_LEN: usize = 192;
const MAX_OPERATION_KEY_LEN: usize = 255;

#[derive(Debug, Clone)]
pub(crate) struct OperationKeyAdapter {
    platform_namespace: Option<String>,
}

impl OperationKeyAdapter {
    pub(crate) fn from_env() -> Result<Self> {
        match env::var(PLATFORM_NAMESPACE_ENV) {
            Ok(namespace) => Self::configured(namespace),
            Err(env::VarError::NotPresent) => Ok(Self::unconfigured()),
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn configured(platform_namespace: impl Into<String>) -> Result<Self> {
        let platform_namespace = platform_namespace.into();
        validate_component("platform namespace", &platform_namespace, MAX_NAMESPACE_LEN)?;
        Ok(Self {
            platform_namespace: Some(platform_namespace),
        })
    }

    pub(crate) fn unconfigured() -> Self {
        Self {
            platform_namespace: None,
        }
    }

    pub(crate) fn inject(&self, params: &Value, mut arguments: Value) -> Result<Value> {
        let arguments = arguments
            .as_object_mut()
            .ok_or_else(|| anyhow!("spend tool arguments must be a JSON object"))?;
        if arguments.contains_key("operation_key") {
            bail!(
                "operation_key is managed by the trusted Hubu MCP adapter and cannot be supplied in model-controlled tool arguments"
            );
        }

        let namespace = self.platform_namespace.as_deref().ok_or_else(|| {
            anyhow!(
                "Hubu MCP spend tools require {PLATFORM_NAMESPACE_ENV}; configure a stable namespace for this agent platform"
            )
        })?;
        let operation_id = params
            .get("_meta")
            .and_then(Value::as_object)
            .and_then(|metadata| metadata.get(OPERATION_METADATA_KEY))
            .and_then(Value::as_object)
            .and_then(|operation| operation.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                anyhow!(
                    "Hubu MCP spend tools require trusted params._meta[\"{OPERATION_METADATA_KEY}\"].id metadata"
                )
            })?;
        validate_component("operation metadata id", operation_id, MAX_OPERATION_ID_LEN)?;

        let operation_key = format!("{namespace}:{operation_id}");
        if operation_key.len() > MAX_OPERATION_KEY_LEN {
            bail!("derived operation key exceeds {MAX_OPERATION_KEY_LEN} bytes");
        }
        arguments.insert("operation_key".to_string(), Value::String(operation_key));
        Ok(arguments.clone().into())
    }
}

fn validate_component(label: &str, value: &str, max_len: usize) -> Result<()> {
    if value.is_empty() {
        bail!("{label} cannot be empty");
    }
    if value.len() > max_len {
        bail!("{label} exceeds {max_len} bytes");
    }
    if !value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
    }) {
        bail!(
            "{label} may contain only ASCII letters, digits, hyphen, underscore, period, colon, and slash"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn params(operation_id: &str) -> Value {
        json!({
            "name": "hubu_authorize_spend",
            "arguments": {
                "account_id": "agent-1",
                "amount_cents": 500,
                "reason": "model call"
            },
            "_meta": {
                OPERATION_METADATA_KEY: {
                    "id": operation_id
                }
            }
        })
    }

    #[test]
    fn injects_a_deterministic_namespaced_operation_key() {
        let adapter = OperationKeyAdapter::configured("codex").unwrap();
        let params = params("tool-call:01JABC");

        let first = adapter
            .inject(&params, params["arguments"].clone())
            .unwrap();
        let retry = adapter
            .inject(&params, params["arguments"].clone())
            .unwrap();

        assert_eq!(first["operation_key"], "codex:tool-call:01JABC");
        assert_eq!(retry, first);
    }

    #[test]
    fn different_platform_operation_ids_produce_different_keys() {
        let adapter = OperationKeyAdapter::configured("codex").unwrap();
        let first_params = params("tool-call:01JABC");
        let second_params = params("tool-call:01JABD");

        let first = adapter
            .inject(&first_params, first_params["arguments"].clone())
            .unwrap();
        let second = adapter
            .inject(&second_params, second_params["arguments"].clone())
            .unwrap();

        assert_ne!(first["operation_key"], second["operation_key"]);
    }

    #[test]
    fn rejects_model_controlled_operation_key_override() {
        let adapter = OperationKeyAdapter::configured("codex").unwrap();
        let params = params("tool-call:01JABC");
        let mut arguments = params["arguments"].clone();
        arguments["operation_key"] = json!("model-invented");

        let error = adapter.inject(&params, arguments).unwrap_err();

        assert!(error.to_string().contains("model-controlled"));
    }

    #[test]
    fn fails_closed_without_trusted_operation_metadata() {
        let adapter = OperationKeyAdapter::configured("codex").unwrap();
        let params = json!({
            "name": "hubu_authorize_spend",
            "arguments": {}
        });

        let error = adapter.inject(&params, json!({})).unwrap_err();

        assert!(error.to_string().contains(OPERATION_METADATA_KEY));
    }

    #[test]
    fn fails_closed_without_a_platform_namespace() {
        let adapter = OperationKeyAdapter::unconfigured();
        let params = params("tool-call:01JABC");

        let error = adapter
            .inject(&params, params["arguments"].clone())
            .unwrap_err();

        assert!(error.to_string().contains(PLATFORM_NAMESPACE_ENV));
    }

    #[test]
    fn rejects_unstable_whitespace_and_control_characters() {
        let adapter = OperationKeyAdapter::configured("codex").unwrap();

        for operation_id in [" tool-call", "tool call", "tool-call\n"] {
            let params = params(operation_id);
            assert!(adapter
                .inject(&params, params["arguments"].clone())
                .is_err());
        }
    }
}
