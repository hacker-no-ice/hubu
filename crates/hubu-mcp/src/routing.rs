use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::{approval_profile, trusted_identity::TrustedSpendIdentity, McpConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubuRequestCapabilityV1 {
    None,
    Approval,
    Reconciliation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HubuHttpRequestV1 {
    pub method: &'static str,
    pub path: String,
    pub body: Option<Value>,
    pub capability: HubuRequestCapabilityV1,
}

#[derive(Debug, Clone, Copy)]
enum HubuResponseTransformV1 {
    Plain,
    SpendApprovalHint,
}

enum PreparedHubuCallV1 {
    Local(Value),
    Http(HubuHttpRequestV1, HubuResponseTransformV1),
}

fn get_request(path: impl Into<String>) -> PreparedHubuCallV1 {
    PreparedHubuCallV1::Http(
        HubuHttpRequestV1 {
            method: "GET",
            path: path.into(),
            body: None,
            capability: HubuRequestCapabilityV1::None,
        },
        HubuResponseTransformV1::Plain,
    )
}

fn post_request(path: &'static str, body: Value) -> PreparedHubuCallV1 {
    post_request_with(
        path,
        body,
        HubuRequestCapabilityV1::None,
        HubuResponseTransformV1::Plain,
    )
}

fn post_request_with(
    path: &'static str,
    body: Value,
    capability: HubuRequestCapabilityV1,
    transform: HubuResponseTransformV1,
) -> PreparedHubuCallV1 {
    PreparedHubuCallV1::Http(
        HubuHttpRequestV1 {
            method: "POST",
            path: path.to_string(),
            body: Some(body),
            capability,
        },
        transform,
    )
}

pub fn route_tool_call_v1(
    params: Value,
    protected_tools_enabled: bool,
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

    let config = McpConfig {
        protected_tools_enabled,
    };
    let prepared = match name {
        "hubu_health" => get_request("/health"),
        "hubu_registration_guidance" => get_request("/registration/guidance"),
        "hubu_client_approval_profile" => PreparedHubuCallV1::Local(approval_profile()),
        "hubu_list_users" => get_request("/users"),
        "hubu_register_human" => {
            require_trusted_client_approval(config, name)?;
            post_request("/init", arguments)
        }
        "hubu_register_agent" => {
            require_trusted_client_approval(config, name)?;
            post_request("/agents/register", arguments)
        }
        "hubu_add_policy" => {
            require_trusted_client_approval(config, name)?;
            post_request("/policies", arguments)
        }
        "hubu_apply_policy" => {
            require_trusted_client_approval(config, name)?;
            let mut arguments = arguments;
            arguments["source"] = json!("mcp");
            post_request("/policies", arguments)
        }
        "hubu_show_policy" => get_request(policy_inspection_path("show", &arguments)?),
        "hubu_export_policy" => get_request(policy_inspection_path("export", &arguments)?),
        "hubu_policy_history" => get_request(policy_inspection_path("history", &arguments)?),
        "hubu_policy_diff" => {
            let mut path = policy_inspection_path("diff", &arguments)?;
            let separator = if path.contains('?') { '&' } else { '?' };
            let from = arguments
                .get("from_revision")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("hubu_policy_diff requires from_revision"))?;
            path.push(separator);
            path.push_str(&format!("from_revision={from}"));
            if let Some(to) = arguments.get("to_revision").and_then(Value::as_u64) {
                path.push_str(&format!("&to_revision={to}"));
            }
            get_request(path)
        }
        "hubu_create_budget" => {
            require_trusted_client_approval(config, name)?;
            post_request("/budgets", arguments)
        }
        "hubu_create_recurring_budget" => {
            require_trusted_client_approval(config, name)?;
            post_request("/budgets/series", arguments)
        }
        "hubu_revoke_budget" => {
            require_trusted_client_approval(config, name)?;
            post_request("/budgets/revoke", arguments)
        }
        "hubu_replace_budget" => {
            require_trusted_client_approval(config, name)?;
            post_request("/budgets/replace", arguments)
        }
        "hubu_set_spending_target" => {
            require_trusted_client_approval(config, name)?;
            post_request("/user/spending-target", arguments)
        }
        "hubu_revoke_spending_target" => {
            require_trusted_client_approval(config, name)?;
            post_request("/user/spending-target/revoke", arguments)
        }
        "hubu_show_spending_targets" => {
            if arguments
                .get("include_all")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                get_request("/user/spending-target?all=true")
            } else {
                get_request("/user/spending-target")
            }
        }
        "hubu_submit_spend" => {
            let arguments = trusted_spend_arguments(&params, arguments)?;
            post_request_with(
                "/spend",
                arguments,
                HubuRequestCapabilityV1::None,
                HubuResponseTransformV1::SpendApprovalHint,
            )
        }
        "hubu_authorize_spend" => {
            let arguments = trusted_spend_arguments(&params, arguments)?;
            post_request_with(
                "/spend/authorize",
                arguments,
                HubuRequestCapabilityV1::None,
                HubuResponseTransformV1::SpendApprovalHint,
            )
        }
        "hubu_get_spend_approval" => {
            let approval_request_id = arguments
                .get("approval_request_id")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("hubu_get_spend_approval requires approval_request_id"))?;
            get_request(format!(
                "/spend/approval?approval_request_id={approval_request_id}"
            ))
        }
        "hubu_resolve_spend_approval" => {
            require_trusted_client_approval(config, name)?;
            post_request_with(
                "/spend/approval/resolve",
                arguments,
                HubuRequestCapabilityV1::Approval,
                HubuResponseTransformV1::SpendApprovalHint,
            )
        }
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
        "hubu_list_ledger" => get_request("/ledger"),
        "hubu_get_executor_claim" => {
            let claim_id = arguments
                .get("claim_id")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("hubu_get_executor_claim requires claim_id"))?;
            get_request(format!("/spend/executor/claim?claim_id={claim_id}"))
        }
        "hubu_list_claims_requiring_reconciliation" => {
            get_request("/spend/executor/reconciliation")
        }
        "hubu_reconcile_vendor_billed_claim" => {
            require_trusted_client_approval(config, name)?;
            post_request_with(
                "/spend/executor/settle",
                arguments,
                HubuRequestCapabilityV1::Reconciliation,
                HubuResponseTransformV1::Plain,
            )
        }
        "hubu_reconcile_vendor_did_not_bill_claim" => {
            require_trusted_client_approval(config, name)?;
            post_request_with(
                "/spend/executor/release",
                arguments,
                HubuRequestCapabilityV1::Reconciliation,
                HubuResponseTransformV1::Plain,
            )
        }
        _ => bail!("unknown Hubu MCP tool `{name}`"),
    };

    let response = match prepared {
        PreparedHubuCallV1::Local(response) => response,
        PreparedHubuCallV1::Http(request, transform) => {
            let response = execute(request)?;
            match transform {
                HubuResponseTransformV1::Plain => response,
                HubuResponseTransformV1::SpendApprovalHint => {
                    spend_response_with_approval_hint(response)
                }
            }
        }
    };
    Ok(tool_result_v1(response))
}

pub(crate) fn trusted_spend_arguments(params: &Value, mut arguments: Value) -> Result<Value> {
    let arguments = arguments
        .as_object_mut()
        .ok_or_else(|| anyhow!("Hubu spend tool arguments must be an object"))?;
    for protected in ["operation_key", "task_id"] {
        if arguments.contains_key(protected) {
            bail!(
                "{protected} is trusted platform state and must not be supplied in model-authored arguments"
            );
        }
    }
    let trusted = TrustedSpendIdentity::from_call_params(params)?;
    arguments.insert(
        "operation_key".to_string(),
        Value::String(trusted.operation_key),
    );
    arguments.insert(
        "task_id".to_string(),
        trusted.task_id.map_or(Value::Null, Value::String),
    );
    Ok(Value::Object(arguments.clone()))
}

fn policy_inspection_path(action: &str, arguments: &Value) -> Result<String> {
    let policy_id = arguments.get("policy_id").and_then(Value::as_str);
    let agent_id = arguments.get("agent_id").and_then(Value::as_str);
    if policy_id.is_some() && agent_id.is_some() {
        bail!("pass only one of policy_id or agent_id");
    }
    let query = policy_id
        .map(|value| format!("?policy_id={value}"))
        .or_else(|| agent_id.map(|value| format!("?agent_id={value}")))
        .unwrap_or_default();
    Ok(format!("/policies/{action}{query}"))
}

pub(crate) fn require_trusted_client_approval(config: McpConfig, tool_name: &str) -> Result<()> {
    if config.protected_tools_enabled {
        Ok(())
    } else {
        Err(anyhow!(
            "{tool_name} requires a trusted MCP client approval gate; set HUBU_MCP_TRUST_CLIENT_APPROVAL=1 only when the MCP client prompts a human before invoking destructive tools"
        ))
    }
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

pub fn tool_result_v1(value: Value) -> Value {
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
