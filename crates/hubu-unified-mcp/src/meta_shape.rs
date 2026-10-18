//! Opt-in sanitized `tools/call` metadata shape diagnostics.
//!
//! Records only key names, JSON types, lengths, and truncated SHA-256 digests
//! so operators can compare the identity metadata that different MCP clients
//! and transports deliver. Tool arguments, raw values, and response content are
//! never recorded. The diagnostic never changes routing or admission.

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::operation_registry::NormalizedHarnessIdentity;

pub(crate) const DIAGNOSTIC_PREFIX: &str = "hubu-unified-mcp meta-shape:";
const DIAGNOSTIC_VERSION: u32 = 1;
const DIGEST_HEX_CHARS: usize = 16;
const MAX_DEPTH: usize = 6;
const MAX_OBJECT_ENTRIES: usize = 64;
const MAX_ARRAY_ELEMENTS: usize = 8;
const MAX_KEY_BYTES: usize = 128;
const MAX_TOOL_NAME_BYTES: usize = 64;

/// Builds one sanitized diagnostic record for a `tools/call` request.
pub(crate) fn record(id: &Value, params: &Value) -> Value {
    let params = params.as_object();
    let meta = params.and_then(|params| params.get("_meta"));
    let identity = match NormalizedHarnessIdentity::from_meta(meta) {
        Ok(identity) => json!({"outcome": "accepted", "platform": identity.platform()}),
        Err(_) => json!({"outcome": "rejected"}),
    };
    let other_params = params
        .map(|params| {
            params
                .keys()
                .filter(|key| !matches!(key.as_str(), "name" | "arguments" | "_meta"))
                .map(|key| Value::String(bounded_key(key)))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "diagnostic_version": DIAGNOSTIC_VERSION,
        "tool": params
            .and_then(|params| params.get("name"))
            .map_or(Value::Null, tool_name),
        "jsonrpc_id": shape(id, 0),
        "arguments_type": params
            .and_then(|params| params.get("arguments"))
            .map_or(Value::Null, |value| Value::String(type_name(value).to_owned())),
        "other_params_keys": other_params,
        "meta": meta.map_or(Value::Null, |meta| shape(meta, 0)),
        "trusted_identity": identity,
    })
}

/// Writes one diagnostic line to stderr, which stdio MCP reserves for logs.
pub(crate) fn emit(id: &Value, params: &Value) {
    eprintln!("{DIAGNOSTIC_PREFIX} {}", record(id, params));
}

fn tool_name(value: &Value) -> Value {
    match value.as_str() {
        Some(name)
            if !name.is_empty()
                && name.len() <= MAX_TOOL_NAME_BYTES
                && name.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                }) =>
        {
            Value::String(name.to_owned())
        }
        _ => Value::String("<unrecognized>".to_owned()),
    }
}

fn shape(value: &Value, depth: usize) -> Value {
    match value {
        Value::Null => json!({"type": "null"}),
        Value::Bool(_) => json!({"type": "boolean"}),
        Value::Number(number) => json!({
            "type": "number",
            "sha256": digest(&number.to_string()),
        }),
        Value::String(text) => json!({
            "type": "string",
            "bytes": text.len(),
            "sha256": digest(text),
        }),
        Value::Array(items) => {
            let mut shaped = json!({"type": "array", "len": items.len()});
            if depth < MAX_DEPTH {
                shaped["items"] = Value::Array(
                    items
                        .iter()
                        .take(MAX_ARRAY_ELEMENTS)
                        .map(|item| shape(item, depth + 1))
                        .collect(),
                );
            }
            shaped
        }
        Value::Object(entries) => {
            let mut shaped = json!({"type": "object", "len": entries.len()});
            if depth < MAX_DEPTH {
                let mut keys = Map::new();
                for (key, item) in entries.iter().take(MAX_OBJECT_ENTRIES) {
                    keys.insert(bounded_key(key), shape(item, depth + 1));
                }
                shaped["keys"] = Value::Object(keys);
            }
            shaped
        }
    }
}

fn bounded_key(key: &str) -> String {
    if key.len() <= MAX_KEY_BYTES {
        return key.to_owned();
    }
    let mut end = MAX_KEY_BYTES;
    while !key.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[{} bytes]", &key[..end], key.len())
}

fn digest(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()[..DIGEST_HEX_CHARS]
        .to_owned()
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_key_names_and_types_without_raw_values() {
        let id = json!("rpc-request-7");
        let params = json!({
            "name": "hubu_submit_spend",
            "arguments": {"reason": "secret prompt text", "amount": "0.03"},
            "_meta": {
                "openai/session": "session-raw-value",
                "progressToken": 42,
                "nested": {"flag": true, "list": ["a", null]}
            },
            "extra": "transport-raw-value"
        });

        let record = record(&id, &params);
        let serialized = record.to_string();

        for raw in [
            "rpc-request-7",
            "secret prompt text",
            "0.03",
            "reason",
            "session-raw-value",
            "transport-raw-value",
        ] {
            assert!(!serialized.contains(raw), "{raw} leaked into {serialized}");
        }
        assert_eq!(record["tool"], "hubu_submit_spend");
        assert_eq!(record["arguments_type"], "object");
        assert_eq!(record["other_params_keys"], json!(["extra"]));
        assert_eq!(record["jsonrpc_id"]["type"], "string");
        let session = &record["meta"]["keys"]["openai/session"];
        assert_eq!(session["type"], "string");
        assert_eq!(session["bytes"], "session-raw-value".len());
        assert_eq!(session["sha256"], digest("session-raw-value"));
        assert_eq!(session["sha256"].as_str().unwrap().len(), DIGEST_HEX_CHARS);
        assert_eq!(record["meta"]["keys"]["progressToken"]["type"], "number");
        assert_eq!(
            record["meta"]["keys"]["nested"]["keys"]["flag"],
            json!({"type": "boolean"})
        );
        assert_eq!(
            record["meta"]["keys"]["nested"]["keys"]["list"]["items"][1],
            json!({"type": "null"})
        );
        assert_eq!(record["trusted_identity"], json!({"outcome": "rejected"}));
    }

    #[test]
    fn digests_are_stable_for_redelivered_values() {
        let params = json!({"name": "hubu_submit_spend", "_meta": {"callId": "call-1"}});
        let first = record(&json!(1), &params);
        let second = record(&json!(2), &params);

        assert_eq!(
            first["meta"]["keys"]["callId"],
            second["meta"]["keys"]["callId"]
        );
        assert_ne!(first["jsonrpc_id"], second["jsonrpc_id"]);
    }

    #[test]
    fn reports_the_accepted_trusted_identity_platform_only() {
        for (meta, platform) in [
            (json!({"callId": "call-raw"}), "codex"),
            (json!({"claudecode/toolUseId": "toolu_raw"}), "claude-code"),
            (
                json!({"hubu.dev/platform-invocation": {
                    "platform": "controlled-test",
                    "invocation_id": "invocation-raw"
                }}),
                "controlled-test",
            ),
        ] {
            let record = record(
                &json!(1),
                &json!({"name": "hubu_authorize_spend", "_meta": meta}),
            );
            assert_eq!(
                record["trusted_identity"],
                json!({"outcome": "accepted", "platform": platform})
            );
            assert!(!record.to_string().contains("raw\""));
        }
    }

    #[test]
    fn missing_meta_and_malformed_params_are_recorded_without_panicking() {
        let missing = record(
            &json!(1),
            &json!({"name": "hubu_submit_spend", "arguments": {}}),
        );
        assert_eq!(missing["meta"], Value::Null);
        assert_eq!(missing["trusted_identity"]["outcome"], "rejected");

        let malformed = record(&Value::Null, &json!("not-an-object"));
        assert_eq!(malformed["tool"], Value::Null);
        assert_eq!(malformed["meta"], Value::Null);
        assert_eq!(malformed["other_params_keys"], json!([]));
    }

    #[test]
    fn output_is_bounded_for_hostile_metadata() {
        let mut deep = json!("leaf-raw");
        for _ in 0..32 {
            deep = json!({"k": deep});
        }
        let wide = (0..500)
            .map(|index| (format!("key-{index}"), json!(index)))
            .collect::<Map<_, _>>();
        let long_key = "x".repeat(4096);
        let params = json!({
            "name": "Hubu Submit\nSpend",
            "_meta": {
                "deep": deep,
                "wide": wide,
                "array": vec![json!(1); 500],
                long_key.clone(): "long-raw"
            }
        });

        let record = record(&json!(1), &params);
        let serialized = record.to_string();

        assert_eq!(record["tool"], "<unrecognized>");
        assert_eq!(record["meta"]["keys"]["wide"]["len"], 500);
        assert_eq!(
            record["meta"]["keys"]["wide"]["keys"]
                .as_object()
                .unwrap()
                .len(),
            MAX_OBJECT_ENTRIES
        );
        assert_eq!(
            record["meta"]["keys"]["array"]["items"]
                .as_array()
                .unwrap()
                .len(),
            MAX_ARRAY_ELEMENTS
        );
        assert!(!serialized.contains(&long_key));
        assert!(!serialized.contains("leaf-raw"));
        assert!(serialized.len() < 16 * 1024, "{} bytes", serialized.len());
    }
}
