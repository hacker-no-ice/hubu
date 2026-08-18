use std::{
    env, fs,
    io::{self, BufRead, Read, Write},
    net::{Shutdown, TcpStream},
};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

mod trusted_identity;

use trusted_identity::TrustedSpendIdentity;

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8787";
const AUTH_TOKEN_ENV: &str = "HUBU_AUTH_TOKEN";
const AUTH_TOKEN_FILE_ENV: &str = "HUBU_AUTH_TOKEN_FILE";
const DEFAULT_AUTH_TOKEN_FILE: &str = "hubu.auth-token";
const RECONCILIATION_TOKEN_ENV: &str = "HUBU_RECONCILIATION_TOKEN";
const RECONCILIATION_TOKEN_FILE_ENV: &str = "HUBU_RECONCILIATION_TOKEN_FILE";
const DEFAULT_RECONCILIATION_TOKEN_FILE: &str = "hubu.reconciliation-token";
const RECONCILIATION_CAPABILITY_HEADER: &str = "X-Hubu-Reconciliation-Capability";
const PROTOCOL_VERSION: &str = "2024-11-05";
const HUBU_APPROVAL_PROFILE_VERSION: &str = "hubu-mcp-client-approval-v1";
const HUBU_MCP_INSTRUCTIONS: &str = "Hubu approval policy: clients should allow read tools and hubu_authorize_spend/hubu_submit_spend without a pre-call human prompt. Every spend call must attach platform-owned operation_key and optional task_id under trusted params._meta['hubu.dev/platform-invocation']; Hubu MCP injects them outside model-authored arguments. The platform remains responsible for stable allocation and retry recovery. Protected setup/admin tools require a human approval prompt before tools/call. Expired-claim reconciliation tools use that prompt gate and a distinct server-verified human reconciliation capability that is never sent on executor requests. If a spend response has requires_human_approval=true, no payment was executed; stop and surface it to the human.";
const READ_TOOL_NAMES: &[&str] = &[
    "hubu_health",
    "hubu_registration_guidance",
    "hubu_client_approval_profile",
    "hubu_list_users",
    "hubu_show_policy",
    "hubu_export_policy",
    "hubu_policy_history",
    "hubu_policy_diff",
    "hubu_list_agents",
    "hubu_list_budgets",
    "hubu_list_ledger",
    "hubu_get_executor_claim",
    "hubu_list_claims_requiring_reconciliation",
];
const SPEND_TOOL_NAMES: &[&str] = &["hubu_authorize_spend", "hubu_submit_spend"];
const APPROVAL_TOOL_NAMES: &[&str] = &[
    "hubu_register_human",
    "hubu_register_agent",
    "hubu_add_policy",
    "hubu_apply_policy",
    "hubu_create_budget",
    "hubu_create_recurring_budget",
    "hubu_reconcile_vendor_billed_claim",
    "hubu_reconcile_vendor_did_not_bill_claim",
];

#[derive(Debug, Clone, Copy)]
struct McpConfig {
    protected_tools_enabled: bool,
}

pub fn run_stdio_from_env() -> Result<()> {
    let base_url = env::var("HUBU_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
    let config = McpConfig {
        protected_tools_enabled: env_flag("HUBU_MCP_TRUST_CLIENT_APPROVAL"),
    };
    run_stdio_with_config(&base_url, config, io::stdin().lock(), io::stdout().lock())
}

pub fn run_stdio(base_url: &str, input: impl BufRead, mut output: impl Write) -> Result<()> {
    run_stdio_with_config(
        base_url,
        McpConfig {
            protected_tools_enabled: false,
        },
        input,
        &mut output,
    )
}

fn run_stdio_with_config(
    base_url: &str,
    config: McpConfig,
    input: impl BufRead,
    mut output: impl Write,
) -> Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = serde_json::from_str(&line)?;
        if let Some(response) = handle_json_rpc(base_url, config, request) {
            writeln!(output, "{}", serde_json::to_string(&response)?)?;
            output.flush()?;
        }
    }
    Ok(())
}

fn env_flag(name: &str) -> bool {
    env::var(name)
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn handle_json_rpc(base_url: &str, config: McpConfig, request: Value) -> Option<Value> {
    let id = request.get("id").cloned();
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "serverInfo": {
                "name": "hubu-mcp-server",
                "version": hubu_common::build::build_info().product_version
            },
            "capabilities": {
                "tools": {}
            },
            "instructions": HUBU_MCP_INSTRUCTIONS
        })),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => {
            let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
            call_tool(base_url, config, params)
        }
        "notifications/initialized" => return None,
        _ => Err(anyhow!("unsupported MCP method `{method}`")),
    };

    let id = id?;
    Some(match result {
        Ok(result) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result
        }),
        Err(error) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32000,
                "message": error.to_string()
            }
        }),
    })
}

fn tool_definitions() -> Vec<Value> {
    vec![
        read_tool(
            "hubu_health",
            "Check whether the local Hubu server is reachable.",
            json_schema(json!({})),
        ),
        read_tool(
            "hubu_registration_guidance",
            "Read compact agent registration guidance.",
            json_schema(json!({})),
        ),
        read_tool(
            "hubu_client_approval_profile",
            "Read Hubu's generic MCP client approval profile for configuring agent harnesses.",
            json_schema(json!({})),
        ),
        read_tool(
            "hubu_list_users",
            "List registered human users and public user ids.",
            json_schema(json!({})),
        ),
        approval_tool(
            "hubu_register_human",
            "Register or select the active human user. Requires a human click.",
            json_schema(json!({
                "username": { "type": "string" },
                "display_name": { "type": "string" },
                "email": { "type": "string" }
            })),
        ),
        approval_tool(
            "hubu_register_agent",
            "Register an agent for an explicit Hubu user. Requires a human click.",
            json_schema(json!({
                "owner_user_id": { "type": "string" },
                "name": { "type": "string" },
                "version": { "type": "string" }
            })),
        ),
        approval_tool(
            "hubu_add_policy",
            "Compatibility alias that declaratively applies and assigns a spending policy. Requires a human click.",
            json_schema(json!({
                "policy_yaml": { "type": "string" },
                "daily_limit_cents": { "type": "integer" }
            })),
        ),
        approval_tool(
            "hubu_apply_policy",
            "Declaratively reconcile a policy resource and assignment with optional compare-and-set. Requires a human click.",
            json_schema_required(json!({
                "policy_yaml": { "type": "string" },
                "declarative_key": { "type": "string" },
                "display_name": { "type": "string" },
                "agent_id": { "type": "string" },
                "expected_revision": { "type": "integer" },
                "expected_hash": { "type": "string" }
            }), &["policy_yaml"]),
        ),
        read_tool(
            "hubu_show_policy",
            "Show complete current policy content and every assignment without database access.",
            json_schema(json!({
                "policy_id": { "type": "string" },
                "agent_id": { "type": "string" }
            })),
        ),
        read_tool(
            "hubu_export_policy",
            "Export the complete current policy as YAML with resource metadata and assignments.",
            json_schema(json!({
                "policy_id": { "type": "string" },
                "agent_id": { "type": "string" }
            })),
        ),
        read_tool(
            "hubu_policy_history",
            "Inspect immutable policy revisions, payload hashes, actors, sources, and timestamps.",
            json_schema(json!({
                "policy_id": { "type": "string" },
                "agent_id": { "type": "string" }
            })),
        ),
        read_tool(
            "hubu_policy_diff",
            "Compare two immutable policy revisions; to_revision defaults to current.",
            json_schema_required(json!({
                "policy_id": { "type": "string" },
                "agent_id": { "type": "string" },
                "from_revision": { "type": "integer" },
                "to_revision": { "type": "integer" }
            }), &["from_revision"]),
        ),
        approval_tool(
            "hubu_create_budget",
            "Create a budget owned by one agent. Requires a human click.",
            json_schema(json!({
                "amount_cents": { "type": "integer" },
                "agent_id": { "type": "string" },
                "starting_at": { "type": "string" },
                "ending_before": { "type": "string" }
            })),
        ),
        approval_tool(
            "hubu_create_recurring_budget",
            "Create a recurring budget series owned by one agent. Requires a human click.",
            json_schema(json!({
                "amount_cents": { "type": "integer" },
                "agent_id": { "type": "string" },
                "recurrence": {
                    "type": "string",
                    "enum": ["daily", "monthly", "yearly"]
                },
                "period_count": { "type": "integer" },
                "starting_at": { "type": "string" }
            })),
        ),
        approval_tool(
            "hubu_revoke_budget",
            "Revoke an active budget. Requires a human click.",
            json_schema(json!({
                "budget_id": { "type": "string" }
            })),
        ),
        approval_tool(
            "hubu_replace_budget",
            "Replace an active budget with a new forward-looking allowance. Requires a human click.",
            json_schema(json!({
                "budget_id": { "type": "string" },
                "amount_cents": { "type": "integer" }
            })),
        ),
        approval_tool(
            "hubu_set_spending_target",
            "Set an advisory spending target for the active Hubu user. Requires a human click.",
            json_schema(json!({
                "amount_cents": { "type": "integer" },
                "starting_at": { "type": "string" },
                "ending_before": { "type": "string" }
            })),
        ),
        approval_tool(
            "hubu_revoke_spending_target",
            "Revoke an advisory spending target for the active Hubu user. Requires a human click.",
            json_schema(json!({
                "target_id": { "type": "string" }
            })),
        ),
        read_tool(
            "hubu_show_spending_targets",
            "Show advisory spending targets and current agent budget allocations for the active Hubu user.",
            json_schema(json!({
                "include_all": { "type": "boolean" }
            })),
        ),
        write_tool(
            "hubu_submit_spend",
            "Submit an agent spend request. Trusted platform metadata supplies operation and optional task identity outside model arguments. Human approval is only required when the returned decision is needs_approval.",
            json_schema_required(json!({
                "account_id": { "type": "string" },
                "amount_cents": { "type": "integer" },
                "reason": { "type": "string" },
                "merchant": { "type": "string" },
                "execution_scope": execution_scope_input_schema(),
                "workload_profile": { "type": "string" }
            }), &["account_id", "amount_cents", "reason"]),
        ),
        write_tool(
            "hubu_authorize_spend",
            "Authorize an agent spend request. Trusted platform metadata supplies operation and optional task identity outside model arguments.",
            json_schema_required(json!({
                "account_id": { "type": "string" },
                "amount_cents": { "type": "integer" },
                "reason": { "type": "string" },
                "merchant": { "type": "string" },
                "execution_scope": execution_scope_input_schema(),
                "workload_profile": { "type": "string" }
            }), &["account_id", "amount_cents", "reason"]),
        ),
        read_tool(
            "hubu_list_agents",
            "List registered agents for the active Hubu user.",
            json_schema(json!({})),
        ),
        read_tool(
            "hubu_list_budgets",
            "List active budgets for the active Hubu user.",
            json_schema(json!({
                "include_all": { "type": "boolean" }
            })),
        ),
        read_tool(
            "hubu_list_ledger",
            "List local ledger transactions.",
            json_schema(json!({})),
        ),
        read_tool(
            "hubu_get_executor_claim",
            "Look up executor claim status, spend scope, hold balance, and reconciliation evidence.",
            json_schema_required(json!({
                "claim_id": { "type": "string" }
            }), &["claim_id"]),
        ),
        read_tool(
            "hubu_list_claims_requiring_reconciliation",
            "List expired executor claims whose budget remains frozen pending human review.",
            json_schema(json!({})),
        ),
        approval_tool(
            "hubu_reconcile_vendor_billed_claim",
            "Confirm after human review that an expired claim was billed and settle its frozen hold. Requires a human click.",
            json_schema_required(json!({
                "claim_id": { "type": "string" },
                "provider_reference": { "type": "string" },
                "evidence": { "type": "string" },
                "receipt": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "actual_vendor_cost_cents": { "type": "integer", "minimum": 0 },
                        "provider_request_id": { "type": "string" },
                        "price_model_snapshot": {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "provider": { "type": "string" },
                                "model": { "type": "string" },
                                "unit_price_cents": { "type": "integer", "minimum": 0 },
                                "pricing_unit": { "type": "string" },
                                "currency": { "type": "string", "enum": ["usd"] }
                            },
                            "required": ["provider", "model", "unit_price_cents", "pricing_unit", "currency"]
                        },
                        "artifact_reference": { "type": "string" }
                    },
                    "required": ["actual_vendor_cost_cents", "provider_request_id", "price_model_snapshot", "artifact_reference"]
                }
            }), &["claim_id", "provider_reference", "evidence", "receipt"]),
        ),
        approval_tool(
            "hubu_reconcile_vendor_did_not_bill_claim",
            "Confirm after human review that an expired claim was not billed and release its frozen hold. Requires a human click.",
            json_schema_required(json!({
                "claim_id": { "type": "string" },
                "provider_reference": { "type": "string" },
                "evidence": { "type": "string" }
            }), &["claim_id", "provider_reference", "evidence"]),
        ),
    ]
}

fn json_schema(properties: Value) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "additionalProperties": false
    })
}

fn execution_scope_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "schema_version": {"type":"integer","const":1},
            "provider": {"type":"string","minLength":1},
            "executor": {"type":"string","minLength":1},
            "capability": {"type":"string","minLength":1},
            "billing_merchant": {"type":"string","minLength":1}
        },
        "required": ["schema_version","provider","executor","capability","billing_merchant"]
    })
}

fn json_schema_required(properties: Value, required: &[&str]) -> Value {
    let mut schema = json_schema(properties);
    schema["required"] = json!(required);
    schema
}

struct ToolAnnotations {
    read_only: bool,
    destructive: bool,
    human_approval: &'static str,
    client_approval_mode: &'static str,
    runtime_approval: &'static str,
}

fn read_tool(name: &str, description: &str, input_schema: Value) -> Value {
    tool(
        name,
        description,
        input_schema,
        ToolAnnotations {
            read_only: true,
            destructive: false,
            human_approval: "none",
            client_approval_mode: "auto",
            runtime_approval: "none",
        },
    )
}

fn write_tool(name: &str, description: &str, input_schema: Value) -> Value {
    tool(
        name,
        description,
        input_schema,
        ToolAnnotations {
            read_only: false,
            destructive: false,
            human_approval: "conditional",
            client_approval_mode: "auto",
            runtime_approval: "hubu_policy_needs_approval",
        },
    )
}

fn approval_tool(name: &str, description: &str, input_schema: Value) -> Value {
    tool(
        name,
        description,
        input_schema,
        ToolAnnotations {
            read_only: false,
            destructive: true,
            human_approval: "required",
            client_approval_mode: "prompt_before_call",
            runtime_approval: "client_human_approval_required",
        },
    )
}

fn tool(name: &str, description: &str, input_schema: Value, annotations: ToolAnnotations) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
        "annotations": {
            "readOnlyHint": annotations.read_only,
            "destructiveHint": annotations.destructive,
            "idempotentHint": false,
            "openWorldHint": true,
            "x_hubu_human_approval": annotations.human_approval,
            "x_hubu_client_approval_mode": annotations.client_approval_mode,
            "x_hubu_runtime_approval": annotations.runtime_approval
        }
    })
}

fn approval_profile() -> Value {
    let auto_approve_tools = READ_TOOL_NAMES
        .iter()
        .chain(SPEND_TOOL_NAMES.iter())
        .copied()
        .collect::<Vec<_>>();
    json!({
        "protocol_version": HUBU_APPROVAL_PROFILE_VERSION,
        "summary": "Configure agent harnesses to auto-call Hubu read and spend tools, prompt before setup/admin tools, and rely on Hubu policy for needs_approval spend outcomes.",
        "client_policy": {
            "auto_approve_tools": auto_approve_tools,
            "prompt_before_call_tools": APPROVAL_TOOL_NAMES,
            "hubu_policy_conditional_tools": SPEND_TOOL_NAMES
        },
        "response_contract": {
            "needs_approval_field": "requires_human_approval",
            "needs_approval_meaning": "Hubu policy required human review and no payment was executed.",
            "agent_action": "Stop the spend workflow and surface approval_reason plus the structured response to the human."
        },
        "annotation_fields": {
            "client_pre_call": "x_hubu_client_approval_mode",
            "runtime_policy": "x_hubu_runtime_approval",
            "legacy_hubu_field": "x_hubu_human_approval"
        },
        "tools": [
            {
                "names": READ_TOOL_NAMES,
                "x_hubu_client_approval_mode": "auto",
                "x_hubu_runtime_approval": "none"
            },
            {
                "names": SPEND_TOOL_NAMES,
                "x_hubu_client_approval_mode": "auto",
                "x_hubu_runtime_approval": "hubu_policy_needs_approval"
            },
            {
                "names": APPROVAL_TOOL_NAMES,
                "x_hubu_client_approval_mode": "prompt_before_call",
                "x_hubu_runtime_approval": "client_human_approval_required"
            }
        ]
    })
}

fn call_tool(base_url: &str, config: McpConfig, params: Value) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("tools/call missing params.name"))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let response = match name {
        "hubu_health" => get_json(base_url, "/health")?,
        "hubu_registration_guidance" => get_json(base_url, "/registration/guidance")?,
        "hubu_client_approval_profile" => approval_profile(),
        "hubu_list_users" => get_json(base_url, "/users")?,
        "hubu_register_human" => {
            require_trusted_client_approval(config, name)?;
            post_json(base_url, "/init", arguments)?
        }
        "hubu_register_agent" => {
            require_trusted_client_approval(config, name)?;
            post_json(base_url, "/agents/register", arguments)?
        }
        "hubu_add_policy" => {
            require_trusted_client_approval(config, name)?;
            post_json(base_url, "/policies", arguments)?
        }
        "hubu_apply_policy" => {
            require_trusted_client_approval(config, name)?;
            let mut arguments = arguments;
            arguments["source"] = json!("mcp");
            post_json(base_url, "/policies", arguments)?
        }
        "hubu_show_policy" => get_json(base_url, &policy_inspection_path("show", &arguments)?)?,
        "hubu_export_policy" => get_json(base_url, &policy_inspection_path("export", &arguments)?)?,
        "hubu_policy_history" => {
            get_json(base_url, &policy_inspection_path("history", &arguments)?)?
        }
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
            get_json(base_url, &path)?
        }
        "hubu_create_budget" => {
            require_trusted_client_approval(config, name)?;
            post_json(base_url, "/budgets", arguments)?
        }
        "hubu_create_recurring_budget" => {
            require_trusted_client_approval(config, name)?;
            post_json(base_url, "/budgets/series", arguments)?
        }
        "hubu_revoke_budget" => {
            require_trusted_client_approval(config, name)?;
            post_json(base_url, "/budgets/revoke", arguments)?
        }
        "hubu_replace_budget" => {
            require_trusted_client_approval(config, name)?;
            post_json(base_url, "/budgets/replace", arguments)?
        }
        "hubu_set_spending_target" => {
            require_trusted_client_approval(config, name)?;
            post_json(base_url, "/user/spending-target", arguments)?
        }
        "hubu_revoke_spending_target" => {
            require_trusted_client_approval(config, name)?;
            post_json(base_url, "/user/spending-target/revoke", arguments)?
        }
        "hubu_show_spending_targets" => {
            if arguments
                .get("include_all")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                get_json(base_url, "/user/spending-target?all=true")?
            } else {
                get_json(base_url, "/user/spending-target")?
            }
        }
        "hubu_submit_spend" => {
            let arguments = trusted_spend_arguments(&params, arguments)?;
            let response = post_json(base_url, "/spend", arguments)?;
            return Ok(tool_result(spend_response_with_approval_hint(response)));
        }
        "hubu_authorize_spend" => {
            let arguments = trusted_spend_arguments(&params, arguments)?;
            let response = post_json(base_url, "/spend/authorize", arguments)?;
            return Ok(tool_result(spend_response_with_approval_hint(response)));
        }
        "hubu_list_agents" => get_json(base_url, "/agents")?,
        "hubu_list_budgets" => {
            if arguments
                .get("include_all")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                get_json(base_url, "/budgets?all=true")?
            } else {
                get_json(base_url, "/budgets")?
            }
        }
        "hubu_list_ledger" => get_json(base_url, "/ledger")?,
        "hubu_get_executor_claim" => {
            let claim_id = arguments
                .get("claim_id")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("hubu_get_executor_claim requires claim_id"))?;
            get_json(
                base_url,
                &format!("/spend/executor/claim?claim_id={claim_id}"),
            )?
        }
        "hubu_list_claims_requiring_reconciliation" => {
            get_json(base_url, "/spend/executor/reconciliation")?
        }
        "hubu_reconcile_vendor_billed_claim" => {
            require_trusted_client_approval(config, name)?;
            post_reconciliation_json(base_url, "/spend/executor/settle", arguments)?
        }
        "hubu_reconcile_vendor_did_not_bill_claim" => {
            require_trusted_client_approval(config, name)?;
            post_reconciliation_json(base_url, "/spend/executor/release", arguments)?
        }
        _ => bail!("unknown Hubu MCP tool `{name}`"),
    };

    Ok(tool_result(response))
}

fn trusted_spend_arguments(params: &Value, mut arguments: Value) -> Result<Value> {
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

fn require_trusted_client_approval(config: McpConfig, tool_name: &str) -> Result<()> {
    if config.protected_tools_enabled {
        Ok(())
    } else {
        Err(anyhow!(
            "{tool_name} requires a trusted MCP client approval gate; set HUBU_MCP_TRUST_CLIENT_APPROVAL=1 only when the MCP client prompts a human before invoking destructive tools"
        ))
    }
}

fn spend_response_with_approval_hint(mut response: Value) -> Value {
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

fn tool_result(value: Value) -> Value {
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

fn get_json(base_url: &str, path: &str) -> Result<Value> {
    request_json(base_url, "GET", path, None, false)
}

fn post_json(base_url: &str, path: &str, body: Value) -> Result<Value> {
    request_json(base_url, "POST", path, Some(body), false)
}

fn post_reconciliation_json(base_url: &str, path: &str, body: Value) -> Result<Value> {
    request_json(base_url, "POST", path, Some(body), true)
}

fn request_json(
    base_url: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    include_reconciliation_capability: bool,
) -> Result<Value> {
    let (host, port) = parse_base_url(base_url)?;
    let body_text = body.map(|body| body.to_string()).unwrap_or_default();
    let authorization_header = auth_token()?
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let reconciliation_header = if include_reconciliation_capability {
        let token = reconciliation_token()?.ok_or_else(|| {
            anyhow!(
                "human reconciliation requires {RECONCILIATION_TOKEN_ENV} or {RECONCILIATION_TOKEN_FILE_ENV}"
            )
        })?;
        format!("{RECONCILIATION_CAPABILITY_HEADER}: {token}\r\n")
    } else {
        String::new()
    };
    let mut stream = TcpStream::connect((host.as_str(), port))
        .with_context(|| format!("connect to Hubu server at {base_url}"))?;

    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\n{authorization_header}{reconciliation_header}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body_text.len(),
        body_text
    )?;
    stream.shutdown(Shutdown::Write)?;

    let mut raw = String::new();
    stream.read_to_string(&mut raw)?;
    let (status, response_body) = parse_http_response(&raw)?;
    let json: Value = serde_json::from_str(response_body)
        .with_context(|| format!("parse server response body `{response_body}`"))?;

    if !(200..300).contains(&status) {
        let message = json
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("request failed");
        bail!("Hubu server returned HTTP {status}: {message}");
    }

    Ok(json)
}

fn parse_base_url(base_url: &str) -> Result<(String, u16)> {
    let trimmed = base_url
        .strip_prefix("http://")
        .ok_or_else(|| anyhow!("only http:// Hubu URLs are supported"))?;
    let host_port = trimmed.trim_end_matches('/');
    let (host, port) = host_port
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("Hubu URL must include a port"))?;
    Ok((host.to_string(), port.parse()?))
}

fn auth_token() -> Result<Option<String>> {
    if let Ok(token) = env::var(AUTH_TOKEN_ENV) {
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err(anyhow!("{AUTH_TOKEN_ENV} cannot be empty"));
        }
        return Ok(Some(token));
    }

    let path =
        env::var(AUTH_TOKEN_FILE_ENV).unwrap_or_else(|_| DEFAULT_AUTH_TOKEN_FILE.to_string());
    match fs::read_to_string(&path) {
        Ok(contents) => {
            let token = contents.trim().to_string();
            if token.is_empty() {
                Err(anyhow!("Hubu auth token file `{path}` is empty"))
            } else {
                Ok(Some(token))
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read Hubu auth token file `{path}`")),
    }
}

fn reconciliation_token() -> Result<Option<String>> {
    if let Ok(token) = env::var(RECONCILIATION_TOKEN_ENV) {
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err(anyhow!("{RECONCILIATION_TOKEN_ENV} cannot be empty"));
        }
        return Ok(Some(token));
    }

    let path = env::var(RECONCILIATION_TOKEN_FILE_ENV)
        .unwrap_or_else(|_| DEFAULT_RECONCILIATION_TOKEN_FILE.to_string());
    match fs::read_to_string(&path) {
        Ok(contents) => {
            let token = contents.trim().to_string();
            if token.is_empty() {
                Err(anyhow!("Hubu reconciliation token file `{path}` is empty"))
            } else {
                Ok(Some(token))
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("read Hubu reconciliation token file `{path}`"))
        }
    }
}

fn parse_http_response(raw: &str) -> Result<(u16, &str)> {
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow!("invalid HTTP response"))?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| anyhow!("invalid HTTP status line"))?
        .parse()?;
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_tools_are_marked_for_human_approval() {
        let tools = tool_definitions();
        let protected = tools
            .iter()
            .find(|tool| tool["name"] == "hubu_create_budget")
            .expect("budget tool should exist");

        assert_eq!(
            protected["annotations"]["x_hubu_human_approval"],
            "required"
        );
        assert_eq!(protected["annotations"]["destructiveHint"], true);
    }

    #[test]
    fn claim_reconciliation_reads_are_automatic_and_resolutions_require_human_approval() {
        let tools = tool_definitions();
        let queue = tools
            .iter()
            .find(|tool| tool["name"] == "hubu_list_claims_requiring_reconciliation")
            .expect("reconciliation queue tool should exist");
        assert_eq!(queue["annotations"]["readOnlyHint"], true);
        assert_eq!(queue["annotations"]["x_hubu_human_approval"], "none");

        for tool_name in [
            "hubu_reconcile_vendor_billed_claim",
            "hubu_reconcile_vendor_did_not_bill_claim",
        ] {
            let resolution = tools
                .iter()
                .find(|tool| tool["name"] == tool_name)
                .expect("reconciliation resolution tool should exist");
            assert_eq!(
                resolution["annotations"]["x_hubu_client_approval_mode"],
                "prompt_before_call"
            );
            assert_eq!(resolution["annotations"]["destructiveHint"], true);
            let required = resolution["inputSchema"]["required"]
                .as_array()
                .expect("resolution fields should be required");
            assert!(required.iter().any(|field| field == "provider_reference"));
            assert!(required.iter().any(|field| field == "evidence"));
            if tool_name == "hubu_reconcile_vendor_billed_claim" {
                assert!(required.iter().any(|field| field == "receipt"));
            }
        }
    }

    #[test]
    fn read_tools_are_marked_read_only() {
        let tools = tool_definitions();
        let guidance = tools
            .iter()
            .find(|tool| tool["name"] == "hubu_registration_guidance")
            .expect("guidance tool should exist");

        assert_eq!(guidance["annotations"]["readOnlyHint"], true);
        assert_eq!(guidance["annotations"]["x_hubu_human_approval"], "none");
        assert_eq!(
            guidance["annotations"]["x_hubu_client_approval_mode"],
            "auto"
        );
    }

    #[test]
    fn spend_needs_approval_adds_human_approval_hint() {
        let response = spend_response_with_approval_hint(json!({
            "decision": "needs_approval",
            "payment": null
        }));

        assert_eq!(response["requires_human_approval"], true);
        assert!(response["approval_reason"].is_string());
    }

    #[test]
    fn authorize_spend_tool_is_agent_callable() {
        let tools = tool_definitions();
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == "hubu_authorize_spend")
            .expect("authorize spend tool should exist");

        assert_eq!(tool["annotations"]["x_hubu_human_approval"], "conditional");
        assert_eq!(tool["annotations"]["x_hubu_client_approval_mode"], "auto");
        assert_eq!(
            tool["annotations"]["x_hubu_runtime_approval"],
            "hubu_policy_needs_approval"
        );
        assert_eq!(tool["annotations"]["destructiveHint"], false);
        assert!(tool["inputSchema"]["properties"]["amount_cents"].is_object());
    }

    #[test]
    fn spend_tool_schemas_keep_trusted_identity_out_of_model_arguments() {
        let tools = tool_definitions();

        for tool_name in ["hubu_submit_spend", "hubu_authorize_spend"] {
            let tool = tools
                .iter()
                .find(|tool| tool["name"] == tool_name)
                .expect("spend tool should exist");
            let properties = &tool["inputSchema"]["properties"];
            let required = tool["inputSchema"]["required"]
                .as_array()
                .expect("spend tool required fields should be an array");

            assert!(properties["account_id"].is_object());
            assert!(properties.get("operation_key").is_none());
            assert!(properties.get("task_id").is_none());
            assert!(properties.get("job_id").is_none());
            assert!(properties.get("agent_id").is_none());
            assert!(required.iter().any(|field| field == "account_id"));
            assert!(!required.iter().any(|field| field == "operation_key"));
            assert!(required.iter().any(|field| field == "amount_cents"));
            assert!(required.iter().any(|field| field == "reason"));
        }
    }

    fn spend_call_params(task_id: Value, arguments: Value) -> Value {
        json!({
            "name": "hubu_authorize_spend",
            "arguments": arguments,
            "_meta": {
                "hubu.dev/platform-invocation": {
                    "platform": "codex",
                    "installation_id": "installation-1",
                    "invocation_id": "call-1",
                    "operation_key": "platform:operation-1",
                    "task_id": task_id
                }
            }
        })
    }

    #[test]
    fn trusted_metadata_is_injected_and_stable_for_retry() {
        let params = spend_call_params(
            json!("linear:HUB-73"),
            json!({
                "account_id": "aga_example",
                "amount_cents": 500,
                "reason": "Generate the review artifact"
            }),
        );
        let first = trusted_spend_arguments(&params, params["arguments"].clone()).unwrap();
        let retry = trusted_spend_arguments(&params, params["arguments"].clone()).unwrap();
        assert_eq!(retry, first);
        assert_eq!(first["operation_key"], "platform:operation-1");
        assert_eq!(first["task_id"], "linear:HUB-73");
        assert_eq!(first["reason"], "Generate the review artifact");
    }

    #[test]
    fn trusted_null_task_id_is_injected_explicitly() {
        let params = spend_call_params(
            Value::Null,
            json!({
                "account_id": "aga_example",
                "amount_cents": 500,
                "reason": "Uncorrelated spend"
            }),
        );
        let forwarded = trusted_spend_arguments(&params, params["arguments"].clone()).unwrap();
        assert!(forwarded["task_id"].is_null());
    }

    #[test]
    fn missing_trusted_task_id_is_normalized_to_explicit_null() {
        let params = json!({
            "name": "hubu_authorize_spend",
            "arguments": {
                "account_id": "aga_example",
                "amount_cents": 500,
                "reason": "Uncorrelated spend"
            },
            "_meta": {
                "hubu.dev/platform-invocation": {
                    "platform": "codex",
                    "installation_id": "installation-1",
                    "invocation_id": "call-1",
                    "operation_key": "platform:operation-1"
                }
            }
        });
        let forwarded = trusted_spend_arguments(&params, params["arguments"].clone()).unwrap();
        assert!(forwarded["task_id"].is_null());
    }

    #[test]
    fn model_cannot_spoof_trusted_operation_or_task_identity() {
        for protected in ["operation_key", "task_id"] {
            let mut arguments = json!({
                "account_id": "aga_example",
                "amount_cents": 500,
                "reason": "Attempt spoof"
            });
            arguments[protected] = json!("spoofed");
            let params = spend_call_params(json!("linear:HUB-73"), arguments.clone());
            let error = trusted_spend_arguments(&params, arguments).unwrap_err();
            assert!(error.to_string().contains("model-authored"));
        }
    }

    #[test]
    fn spend_without_trusted_identity_fails_closed() {
        let params = json!({
            "name": "hubu_authorize_spend",
            "arguments": {
                "account_id": "aga_example",
                "amount_cents": 500,
                "reason": "Missing trusted identity"
            }
        });
        let error = trusted_spend_arguments(&params, params["arguments"].clone()).unwrap_err();
        assert!(error.to_string().contains("hubu.dev/platform-invocation"));
    }

    #[test]
    fn protected_tool_requires_client_prompt_annotation() {
        let tools = tool_definitions();
        let protected = tools
            .iter()
            .find(|tool| tool["name"] == "hubu_register_agent")
            .expect("agent registration tool should exist");

        assert_eq!(
            protected["annotations"]["x_hubu_client_approval_mode"],
            "prompt_before_call"
        );
        assert_eq!(
            protected["annotations"]["x_hubu_runtime_approval"],
            "client_human_approval_required"
        );
    }

    #[test]
    fn approval_profile_lists_spend_tools_as_auto_with_hubu_policy_runtime() {
        let profile = approval_profile();

        assert_eq!(profile["protocol_version"], HUBU_APPROVAL_PROFILE_VERSION);
        assert!(profile["client_policy"]["auto_approve_tools"]
            .as_array()
            .expect("auto tools should be an array")
            .iter()
            .any(|tool| tool == "hubu_submit_spend"));
        assert!(profile["client_policy"]["prompt_before_call_tools"]
            .as_array()
            .expect("prompt tools should be an array")
            .iter()
            .any(|tool| tool == "hubu_create_budget"));
        assert_eq!(
            profile["response_contract"]["needs_approval_field"],
            "requires_human_approval"
        );
    }

    #[test]
    fn initialize_includes_client_approval_instructions() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize"
        });

        let response = handle_json_rpc(
            DEFAULT_BASE_URL,
            McpConfig {
                protected_tools_enabled: false,
            },
            request,
        )
        .expect("initialize should return a response");

        let instructions = response["result"]["instructions"]
            .as_str()
            .expect("instructions should be present");
        assert!(instructions.contains("hubu_submit_spend"));
        assert!(instructions.contains("Protected setup/admin tools require"));
        assert_eq!(
            response["result"]["serverInfo"]["version"],
            hubu_common::build::build_info().product_version
        );
    }

    #[test]
    fn protected_tool_schema_does_not_accept_agent_controlled_approval() {
        let tools = tool_definitions();
        let protected = tools
            .iter()
            .find(|tool| tool["name"] == "hubu_create_budget")
            .expect("budget tool should exist");

        assert!(protected["inputSchema"]["properties"]
            .get("human_approved")
            .is_none());
    }

    #[test]
    fn protected_tool_rejects_without_trusted_client_gate() {
        let error = require_trusted_client_approval(
            McpConfig {
                protected_tools_enabled: false,
            },
            "hubu_create_budget",
        )
        .expect_err("protected tool should require trusted client gate");

        assert!(error.to_string().contains("trusted MCP client approval"));
    }
}
