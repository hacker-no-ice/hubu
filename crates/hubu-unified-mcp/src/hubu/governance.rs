//! Read-only adapter for the Hubu governance HTTP contract. The backend owns
//! identity scope, durable token verification, aggregation and coverage.
use anyhow::{anyhow, bail, Result};
use chrono::DateTime;
use serde_json::{json, Map, Value};

pub(super) const TOOL_NAMES: [&str; 3] = [
    "hubu_governance_summary",
    "hubu_list_governance_records",
    "hubu_get_governance_record",
];
const CATEGORIES: &[&str] = &[
    "initial_policy",
    "automated_admission",
    "admission_failure",
    "human_escalation",
    "human_resolution",
    "authorization",
    "authorization_release",
    "execution_start",
    "execution_failure",
    "execution_finalization",
    "reconciliation_entry",
    "reconciliation_resolution",
];
const OUTCOMES: &[&str] = &[
    "policy_allow",
    "policy_deny",
    "policy_needs_approval",
    "automatic_allow",
    "automatic_deny",
    "admission_failed",
    "escalated",
    "human_approved",
    "human_declined",
    "authorized",
    "authorization_released",
    "execution_started",
    "execution_failed",
    "execution_settled",
    "execution_released",
    "reconciliation_required",
    "reconciliation_resolved",
];
const COMMON_FIELDS: &[&str] = &[
    "agent_id",
    "account_id",
    "categories",
    "outcomes",
    "occurred_from",
    "occurred_before",
    "snapshot",
];

pub(super) fn is_tool(name: &str) -> bool {
    TOOL_NAMES.contains(&name)
}

pub(super) fn input_schema(name: &str) -> Value {
    let mut properties = json!({
        "agent_id": public_id_schema("agt_"),
        "account_id": public_id_schema("aga_"),
        "categories": {"type":"array", "minItems":1, "items":{"type":"string", "enum":CATEGORIES}},
        "outcomes": {"type":"array", "minItems":1, "items":{"type":"string", "enum":OUTCOMES}},
        "occurred_from": {"type":"string", "format":"date-time", "description":"Inclusive RFC3339 occurrence instant. Use an explicit UTC boundary for period questions."},
        "occurred_before": {"type":"string", "format":"date-time", "description":"Exclusive RFC3339 occurrence instant, later than occurred_from."},
        "snapshot": {"type":"string", "minLength":1, "maxLength":2048, "description":"Copy summary.snapshot.token unchanged as a raw string, with the same filters. Do not parse or URL-encode it."}
    });
    if name == "hubu_list_governance_records" {
        properties["limit"] = json!({"type":"integer", "minimum":1, "maximum":200});
        properties["cursor"] = json!({"type":"string", "minLength":1, "maxLength":4096, "description":"Copy next_cursor unchanged as a raw string; it already carries the snapshot."});
    }
    if name == "hubu_get_governance_record" {
        properties["record_id"] = public_id_schema("lev_");
    }
    let mut schema =
        json!({"type":"object", "properties":properties, "additionalProperties":false});
    if name == "hubu_get_governance_record" {
        schema["required"] = json!(["record_id"]);
    }
    schema
}

fn public_id_schema(prefix: &str) -> Value {
    json!({"type":"string", "minLength":5, "maxLength":100, "pattern":format!("^{prefix}[A-Za-z0-9_-]+$")})
}

fn safe_public_id(value: &str, prefix: &str) -> bool {
    value.len() <= 100
        && value.strip_prefix(prefix).is_some_and(|suffix| {
            !suffix.is_empty()
                && suffix
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
        })
}

pub(super) fn is_read_path(path: &str) -> bool {
    matches!(path, "/governance/summary" | "/governance/records")
        || path
            .strip_prefix("/governance/records/")
            .is_some_and(|id| safe_public_id(id, "lev_"))
}

/// Validate model arguments before dispatch. Tokens stay opaque here; Hubu
/// checks their owned boundary and normalized filters against durable records.
pub(super) fn request_path(name: &str, arguments: &Value) -> Result<String> {
    let args = arguments
        .as_object()
        .ok_or_else(|| anyhow!("governance arguments must be an object"))?;
    if args.keys().any(|key| {
        !(COMMON_FIELDS.contains(&key.as_str())
            || name == "hubu_list_governance_records" && matches!(key.as_str(), "cursor" | "limit")
            || name == "hubu_get_governance_record" && key == "record_id")
    }) {
        bail!("unsupported governance argument");
    }
    let endpoint = match name {
        "hubu_governance_summary" => "/governance/summary".to_string(),
        "hubu_list_governance_records" => "/governance/records".to_string(),
        "hubu_get_governance_record" => {
            let id = nonempty_string(args, "record_id", 100)?
                .ok_or_else(|| anyhow!("governance detail requires record_id"))?;
            if !safe_public_id(id, "lev_") {
                bail!("record_id must be a safe public lifecycle identifier");
            }
            format!("/governance/records/{id}")
        }
        _ => bail!("unsupported governance tool"),
    };
    let mut url = reqwest::Url::parse(&format!("http://localhost{endpoint}"))?;
    for (key, prefix) in [("agent_id", "agt_"), ("account_id", "aga_")] {
        if let Some(id) = nonempty_string(args, key, 100)? {
            if !safe_public_id(id, prefix) {
                bail!("{key} must be a safe public identifier");
            }
            url.query_pairs_mut().append_pair(key, id);
        }
    }
    for (key, supported) in [("categories", CATEGORIES), ("outcomes", OUTCOMES)] {
        if let Some(value) = args.get(key) {
            let items = value
                .as_array()
                .filter(|items| !items.is_empty())
                .ok_or_else(|| anyhow!("{key} must be a nonempty array of supported names"))?;
            let values = items
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .filter(|value| supported.contains(value))
                        .ok_or_else(|| anyhow!("{key} must contain only supported names"))
                })
                .collect::<Result<Vec<_>>>()?;
            url.query_pairs_mut().append_pair(key, &values.join(","));
        }
    }
    let from = time_argument(args, "occurred_from")?;
    let before = time_argument(args, "occurred_before")?;
    if from
        .zip(before)
        .is_some_and(|(from, before)| from >= before)
    {
        bail!("occurred_from must be before occurred_before");
    }
    for key in ["occurred_from", "occurred_before"] {
        if let Some(value) = args.get(key).and_then(Value::as_str) {
            url.query_pairs_mut().append_pair(key, value);
        }
    }
    if let Some(snapshot) = nonempty_string(args, "snapshot", 2048)? {
        url.query_pairs_mut().append_pair("snapshot", snapshot);
    }
    if let Some(value) = args.get("limit") {
        let limit = value
            .as_u64()
            .filter(|n| (1..=200).contains(n))
            .ok_or_else(|| anyhow!("limit must be an integer between 1 and 200"))?;
        url.query_pairs_mut()
            .append_pair("limit", &limit.to_string());
    }
    if let Some(cursor) = nonempty_string(args, "cursor", 4096)? {
        url.query_pairs_mut().append_pair("cursor", cursor);
    }
    Ok(format!(
        "{}{}",
        url.path(),
        url.query().map(|q| format!("?{q}")).unwrap_or_default()
    ))
}

fn nonempty_string<'a>(
    args: &'a Map<String, Value>,
    key: &str,
    max_bytes: usize,
) -> Result<Option<&'a str>> {
    args.get(key)
        .map(|value| {
            value
                .as_str()
                .filter(|s| !s.trim().is_empty() && s.len() <= max_bytes)
                .ok_or_else(|| anyhow!("{key} must be a nonempty string within its size limit"))
        })
        .transpose()
}

fn time_argument(
    args: &Map<String, Value>,
    key: &str,
) -> Result<Option<DateTime<chrono::FixedOffset>>> {
    nonempty_string(args, key, 100)?
        .map(|value| {
            DateTime::parse_from_rfc3339(value)
                .map_err(|_| anyhow!("{key} must be an RFC3339 instant"))
        })
        .transpose()
}
