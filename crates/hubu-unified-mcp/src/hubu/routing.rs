use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::operation_registry::OperationResolution;

pub(crate) const DENIED_OPERATION_GUIDANCE: &str = "This denied operation is terminal. Exact redelivery only recovers the same denial. Submit corrected work as a new tool call so the harness creates a new logical operation.";

pub(crate) fn denied_retry_guidance() -> Value {
    json!({
        "action": "create_new_operation",
        "message": DENIED_OPERATION_GUIDANCE,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct HubuHttpRequestV1 {
    pub method: &'static str,
    pub path: String,
    pub body: Option<Value>,
}

#[derive(Debug, Clone, Copy)]
enum HubuResponseTransformV1 {
    Plain,
    SpendApprovalHint,
    PrivateOperationIdentity,
    SpendApprovalPrompt,
}

enum PreparedHubuCallV1 {
    Http(HubuHttpRequestV1, HubuResponseTransformV1),
}

fn get_request(path: impl Into<String>) -> PreparedHubuCallV1 {
    PreparedHubuCallV1::Http(
        HubuHttpRequestV1 {
            method: "GET",
            path: path.into(),
            body: None,
        },
        HubuResponseTransformV1::Plain,
    )
}

fn post_request(path: impl Into<String>, body: Value) -> PreparedHubuCallV1 {
    post_request_with(path, body, HubuResponseTransformV1::Plain)
}

fn post_request_with(
    path: impl Into<String>,
    body: Value,
    transform: HubuResponseTransformV1,
) -> PreparedHubuCallV1 {
    PreparedHubuCallV1::Http(
        HubuHttpRequestV1 {
            method: "POST",
            path: path.into(),
            body: Some(body),
        },
        transform,
    )
}

pub(super) fn route_tool_call_v1(
    params: Value,
    operation: Option<OperationResolution>,
    execute: impl FnOnce(HubuHttpRequestV1) -> Result<Value>,
) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("tools/call missing params.name"))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let prepared = match name {
        "hubu_registration_guidance" => get_request("/registration/guidance"),
        "hubu_register_agent" => post_request("/agents/register", arguments),
        "hubu_show_policy" => {
            let fields = arguments
                .as_object()
                .ok_or_else(|| anyhow!("hubu_show_policy requires an object"))?;
            if fields
                .keys()
                .any(|key| !matches!(key.as_str(), "policy_id" | "agent_id" | "include_yaml"))
            {
                bail!("hubu_show_policy received an unknown field");
            }
            match arguments.get("include_yaml") {
                None | Some(Value::Bool(false)) => {}
                Some(Value::Bool(true)) => return Ok(owner_action_result("Policy YAML export")),
                Some(_) => bail!("hubu_show_policy include_yaml must be a boolean"),
            }
            if fields.contains_key("policy_id") && fields.contains_key("agent_id") {
                bail!("pass only one of policy_id or agent_id");
            }
            let mut selectors = fields.clone();
            selectors.remove("include_yaml");
            get_request(history_path(
                "/policies/show",
                &Value::Object(selectors),
                &["policy_id", "agent_id"],
            )?)
        }
        "hubu_submit_spend" => {
            let arguments = trusted_spend_arguments(arguments, operation.as_ref())?;
            post_request_with(
                "/spend",
                arguments,
                HubuResponseTransformV1::SpendApprovalHint,
            )
        }
        "hubu_authorize_spend" => {
            let arguments = trusted_spend_arguments(arguments, operation.as_ref())?;
            post_request_with(
                "/spend/authorize",
                arguments,
                HubuResponseTransformV1::SpendApprovalHint,
            )
        }
        "hubu_get_spend_approval" => {
            let approval_request_id = spend_approval_request_id(&arguments, name)?;
            PreparedHubuCallV1::Http(
                HubuHttpRequestV1 {
                    method: "GET",
                    path: format!("/spend/approval?approval_request_id={approval_request_id}"),
                    body: None,
                },
                HubuResponseTransformV1::PrivateOperationIdentity,
            )
        }
        "hubu_prompt_spend_approval" => post_request_with(
            "/spend/approval/prompt",
            json!({"approval_request_id": spend_approval_request_id(&arguments, name)?}),
            HubuResponseTransformV1::SpendApprovalPrompt,
        ),
        "hubu_list_agents" => get_request("/agents"),
        "hubu_list_budgets" => {
            if arguments
                .get("include_all")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                get_request("/budgets?all=true")
            } else {
                get_request("/budgets")
            }
        }
        "hubu_list_ledger" => get_request(history_path(
            "/ledger/transactions",
            &arguments,
            &["agent_id", "account_id", "budget_id", "limit", "cursor"],
        )?),
        "hubu_list_authorization_records" => get_request(history_path(
            "/spend/authorizations",
            &arguments,
            &["agent_id", "account_id", "status", "limit", "cursor"],
        )?),
        "hubu_get_authorization_record" => {
            if arguments
                .get("authorization_id")
                .and_then(Value::as_str)
                .is_none()
            {
                bail!("hubu_get_authorization_record requires authorization_id");
            }
            get_request(history_path(
                "/spend/authorizations/show",
                &arguments,
                &["authorization_id"],
            )?)
        }
        _ => bail!("unknown Hubu MCP tool `{name}`"),
    };

    let response = match prepared {
        PreparedHubuCallV1::Http(request, transform) => {
            let response = execute(request)?;
            match transform {
                HubuResponseTransformV1::Plain => response,
                HubuResponseTransformV1::SpendApprovalHint => {
                    spend_response_with_approval_hint(response)
                }
                HubuResponseTransformV1::PrivateOperationIdentity => {
                    redact_private_operation_identity(response, None)
                }
                HubuResponseTransformV1::SpendApprovalPrompt => {
                    redact_authorization_continuation(redact_private_operation_identity(
                        spend_response_with_approval_hint(response),
                        None,
                    ))
                }
            }
        }
    };
    Ok(tool_result_v1(response))
}

pub(crate) fn trusted_spend_arguments(
    mut arguments: Value,
    operation: Option<&OperationResolution>,
) -> Result<Value> {
    validate_model_spend_arguments(&arguments)?;
    let arguments = arguments
        .as_object_mut()
        .ok_or_else(|| anyhow!("Hubu spend tool arguments must be an object"))?;
    let trusted = operation.ok_or_else(|| {
        anyhow!("Hubu spend tools require a resolved trusted harness operation identity")
    })?;
    let operation_key = trusted
        .operation_key
        .as_ref()
        .ok_or_else(|| anyhow!("normalized operation no longer requires backend dispatch"))?;
    arguments.insert(
        "operation_key".to_string(),
        Value::String(operation_key.clone()),
    );
    arguments.insert(
        "task_id".to_string(),
        trusted.task_id.clone().map_or(Value::Null, Value::String),
    );
    Ok(Value::Object(arguments.clone()))
}

pub(crate) fn validate_model_spend_arguments(arguments: &Value) -> Result<()> {
    let arguments = arguments
        .as_object()
        .ok_or_else(|| anyhow!("Hubu spend tool arguments must be an object"))?;
    for protected in [
        "operation_key",
        "operation_handle",
        "task_id",
        "platform",
        "installation_id",
        "invocation_id",
        "call_id",
        "callId",
        "tool_use_id",
        "claudecode/toolUseId",
        "_meta",
    ] {
        if arguments.contains_key(protected) {
            bail!(
                "{protected} is trusted platform state and must not be supplied in model-authored arguments"
            );
        }
    }
    if arguments.contains_key("amount_cents") {
        bail!("amount_cents has been removed; provide amount as a decimal string");
    }
    Ok(())
}

pub(crate) fn public_spend_result(
    mut response: Value,
    operation_handle: &str,
    private_operation_key: Option<&str>,
) -> Value {
    response = redact_private_operation_identity(response, private_operation_key);
    if let Some(object) = response.as_object_mut() {
        let denied = object.get("decision").and_then(Value::as_str) == Some("deny");
        object.insert(
            "operation_handle".to_string(),
            Value::String(operation_handle.to_string()),
        );
        if denied {
            object.insert("retry_guidance".to_string(), denied_retry_guidance());
        }
        object.insert(
            "agent_guidance".to_string(),
            if denied {
                json!({
                    "on_ambiguous_result": "redeliver_exact_call",
                    "on_denied_result": "create_new_operation",
                    "replacement_call": "create_new_operation",
                    "message": DENIED_OPERATION_GUIDANCE,
                })
            } else {
                json!({
                    "on_ambiguous_result": "redeliver_exact_call",
                    "replacement_call": "do_not_submit",
                    "message": "If the client-visible result is ambiguous, redeliver this exact harness call with the same call identity; do not submit a replacement spend call."
                })
            },
        );
    }
    response
}

pub(crate) fn redact_private_operation_identity(
    mut value: Value,
    private_operation_key: Option<&str>,
) -> Value {
    let mut private_operation_keys = Vec::new();
    collect_private_operation_keys(&value, &mut private_operation_keys);
    if let Some(operation_key) = private_operation_key.filter(|value| !value.is_empty()) {
        private_operation_keys.push(operation_key.to_string());
    }
    private_operation_keys
        .sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    private_operation_keys.dedup();
    remove_private_operation_identity(&mut value, &private_operation_keys);
    value
}

fn redact_authorization_continuation(mut value: Value) -> Value {
    let mut private_continuations = Vec::new();
    collect_authorization_continuations(&value, &mut private_continuations);
    private_continuations
        .sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    private_continuations.dedup();
    remove_authorization_continuations(&mut value, &private_continuations);
    value
}

fn collect_authorization_continuations(value: &Value, private_continuations: &mut Vec<String>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_authorization_continuations(value, private_continuations);
            }
        }
        Value::Object(object) => {
            for field in ["auth_token_id", "spend_auth_token_id"] {
                if let Some(continuation) = object.get(field).and_then(Value::as_str) {
                    if !continuation.is_empty() {
                        private_continuations.push(continuation.to_string());
                    }
                }
            }
            for value in object.values() {
                collect_authorization_continuations(value, private_continuations);
            }
        }
        _ => {}
    }
}

fn remove_authorization_continuations(value: &mut Value, private_continuations: &[String]) {
    match value {
        Value::Array(values) => {
            for value in values {
                remove_authorization_continuations(value, private_continuations);
            }
        }
        Value::Object(object) => {
            object.remove("auth_token_id");
            object.remove("spend_auth_token_id");
            for value in object.values_mut() {
                remove_authorization_continuations(value, private_continuations);
            }
        }
        Value::String(text) => {
            *text = redact_authorization_continuation_text(text, private_continuations);
        }
        _ => {}
    }
}

fn redact_authorization_continuation_text(text: &str, private_continuations: &[String]) -> String {
    let mut redacted = String::with_capacity(text.len());
    let mut offset = 0;
    while offset < text.len() {
        if let Some(continuation) = private_continuations
            .iter()
            .find(|continuation| text[offset..].starts_with(continuation.as_str()))
        {
            redacted.push_str("<private authorization redacted>");
            offset += continuation.len();
        } else {
            let character = text[offset..]
                .chars()
                .next()
                .expect("offset should remain on a character boundary");
            redacted.push(character);
            offset += character.len_utf8();
        }
    }
    redacted
}

fn collect_private_operation_keys(value: &Value, private_operation_keys: &mut Vec<String>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_private_operation_keys(value, private_operation_keys);
            }
        }
        Value::Object(object) => {
            if let Some(operation_key) = object.get("operation_key").and_then(Value::as_str) {
                if !operation_key.is_empty() {
                    private_operation_keys.push(operation_key.to_string());
                }
            }
            for value in object.values() {
                collect_private_operation_keys(value, private_operation_keys);
            }
        }
        _ => {}
    }
}

fn remove_private_operation_identity(value: &mut Value, private_operation_keys: &[String]) {
    match value {
        Value::Array(values) => {
            for value in values {
                remove_private_operation_identity(value, private_operation_keys);
            }
        }
        Value::Object(object) => {
            object.remove("operation_key");
            for value in object.values_mut() {
                remove_private_operation_identity(value, private_operation_keys);
            }
        }
        Value::String(text) => {
            *text = redact_private_operation_key_text(text, private_operation_keys);
        }
        _ => {}
    }
}

fn redact_private_operation_key_text(text: &str, private_operation_keys: &[String]) -> String {
    let mut redacted = String::with_capacity(text.len());
    let mut offset = 0;
    while offset < text.len() {
        if let Some(operation_key) = private_operation_keys
            .iter()
            .find(|operation_key| text[offset..].starts_with(operation_key.as_str()))
        {
            redacted.push_str("<private operation redacted>");
            offset += operation_key.len();
        } else {
            let character = text[offset..]
                .chars()
                .next()
                .expect("offset should remain on a character boundary");
            redacted.push(character);
            offset += character.len_utf8();
        }
    }
    redacted
}

fn spend_approval_request_id(arguments: &Value, tool_name: &str) -> Result<String> {
    let arguments = arguments
        .as_object()
        .ok_or_else(|| anyhow!("{tool_name} arguments must be an object"))?;
    if arguments.len() != 1 {
        bail!("{tool_name} accepts only its documented approval fields");
    }
    let approval_request_id = arguments
        .get("approval_request_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("{tool_name} requires approval_request_id"))?;
    if !crate::is_safe_approval_request_id(approval_request_id) {
        bail!("{tool_name} requires approval_request_id to be a safe identifier");
    }
    Ok(approval_request_id.to_string())
}

pub(crate) fn spend_response_with_approval_hint(mut response: Value) -> Value {
    let requires_human_approval = response
        .get("decision")
        .and_then(Value::as_str)
        .map(|decision| decision == "needs_approval")
        .unwrap_or(false);
    if let Some(object) = response.as_object_mut() {
        object.insert(
            "requires_human_approval".to_string(),
            Value::Bool(requires_human_approval),
        );
        if requires_human_approval {
            object.insert(
                "approval_reason".to_string(),
                Value::String(
                    "policy returned needs_approval; Hubu did not execute payment".to_string(),
                ),
            );
        }
    }
    response
}

pub(super) fn tool_result_v1(value: Value) -> Value {
    json!({
        "content": [
            {
                "type": "text",
                "text": serde_json::to_string_pretty(&value)
                    .expect("tool response should serialize")
            }
        ],
        "structuredContent": value
    })
}

pub(crate) fn owner_action_result(action: &str) -> Value {
    tool_error_result_v1(json!({
        "error_code": "owner_authority_required",
        "error": format!("{action} is an owner action. Use the Hubu admin portal."),
        "retry_guidance": {"action": "ask_owner"},
    }))
}

pub(super) fn tool_error_result_v1(value: Value) -> Value {
    json!({
        "content": [
            {
                "type": "text",
                "text": serde_json::to_string_pretty(&value)
                    .expect("tool error response should serialize")
            }
        ],
        "structuredContent": value,
        "isError": true
    })
}

fn history_path(endpoint: &str, arguments: &Value, allowed: &[&str]) -> Result<String> {
    let args = arguments
        .as_object()
        .ok_or_else(|| anyhow!("history arguments must be an object"))?;
    if args.keys().any(|key| !allowed.contains(&key.as_str())) {
        bail!("unsupported history argument");
    }
    if args.contains_key("budget_id") && !args.contains_key("agent_id") {
        bail!("budget_id requires agent_id");
    }
    let mut url = reqwest::Url::parse(&format!("http://localhost{endpoint}"))?;
    for key in allowed {
        if let Some(value) = args.get(*key) {
            let value = if *key == "limit" {
                value
                    .as_u64()
                    .filter(|n| (1..=100).contains(n))
                    .ok_or_else(|| anyhow!("limit must be between 1 and 100"))?
                    .to_string()
            } else {
                value
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| anyhow!("{key} must be a nonempty string"))?
                    .to_string()
            };
            if *key == "status"
                && !matches!(
                    value.as_str(),
                    "authorized"
                        | "unknown"
                        | "needs_approval"
                        | "claimed"
                        | "settled"
                        | "released"
                        | "expired"
                        | "reconciliation_required"
                )
            {
                bail!("invalid authorization record status");
            }
            url.query_pairs_mut().append_pair(key, &value);
        }
    }
    Ok(format!(
        "{}{}",
        url.path(),
        url.query().map(|q| format!("?{q}")).unwrap_or_default()
    ))
}
